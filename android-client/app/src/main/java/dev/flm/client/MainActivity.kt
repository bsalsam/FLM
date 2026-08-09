package dev.flm.client

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.TextView
import android.widget.Toast
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.NetworkInterface
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

private const val TAG = "FlmClient"
private const val UDP_PORT = 5000
private const val CONTROL_PORT = 5001

/**
 * Resolução usada quando o daemon não informa nada pelo canal de controle
 * (ex.: `flm-daemon <ip>` no modo one-shot, que não abre o canal). Mantém o
 * comportamento antigo funcionando em vez de deixar a tela preta para sempre.
 */
private const val FALLBACK_WIDTH = 1024
private const val FALLBACK_HEIGHT = 768

/** Quanto esperar pelo canal de controle antes de cair no fallback. */
private const val CONTROL_WAIT_MS = 4000

/**
 * Recebe RTP/H.264 vindo do daemon Linux e decodifica via MediaCodec (modo
 * assíncrono) direto numa Surface. Sem jitter buffer real e sem mDNS/pareamento
 * ainda. Ver docs/plano-arquitetura.md.
 *
 * A resolução do decoder é informada pelo daemon num canal de controle TCP
 * (porta 5001) antes de o vídeo começar a fluir: o decoder de hardware Qualcomm
 * não renegocia dimensões pelo SPS -- com mismatch ele simplesmente não produz
 * imagem --, então precisa ser criado já com o tamanho certo. Ver control.rs no
 * daemon.
 */
class MainActivity : Activity(), SurfaceHolder.Callback {

    private lateinit var surfaceView: SurfaceView

    private val nalQueue = LinkedBlockingQueue<ByteArray>()
    private var receiver: RtpReceiver? = null
    private var control: ControlServer? = null

    private var decoder: MediaCodec? = null
    private var decoderWidth = 0
    private var decoderHeight = 0

    /** Surface válida entre surfaceCreated e surfaceDestroyed. */
    private var holder: SurfaceHolder? = null

    // Tela de espera: some no primeiro frame renderizado, volta se a surface
    // for recriada (app saiu e voltou pro primeiro plano).
    private lateinit var splash: View
    private lateinit var ipWifiView: TextView
    private lateinit var ipUsbView: TextView
    private val uiHandler = Handler(Looper.getMainLooper())
    private val primeiroFrame = AtomicBoolean(false)
    private val atualizarIps = object : Runnable {
        override fun run() {
            // getNetworkInterfaces pode bloquear; roda fora da UI thread.
            Thread {
                val (wifi, usb) = enderecosIp()
                runOnUiThread {
                    ipWifiView.text = wifi ?: "—"
                    ipUsbView.text = usb ?: "—"
                }
            }.start()
            uiHandler.postDelayed(this, 2000)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Monitor secundário não deve apagar a tela no meio do uso.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        setContentView(R.layout.activity_main)
        surfaceView = findViewById(R.id.surface_view)
        surfaceView.holder.addCallback(this)

        splash = findViewById(R.id.splash_overlay)
        ipWifiView = findViewById(R.id.ip_wifi)
        ipUsbView = findViewById(R.id.ip_usb)
        findViewById<TextView>(R.id.splash_version).text =
            "versão ${packageManager.getPackageInfo(packageName, 0).versionName}"
        configurarBotaoCopiar(findViewById(R.id.btn_copy_wifi), ipWifiView, "IP Wi-Fi")
        configurarBotaoCopiar(findViewById(R.id.btn_copy_usb), ipUsbView, "IP USB")
    }

    private fun configurarBotaoCopiar(botao: Button, origem: TextView, rotulo: String) {
        botao.setOnClickListener {
            val ip = origem.text.toString()
            if (ip == "—") {
                Toast.makeText(this, "$rotulo indisponível", Toast.LENGTH_SHORT).show()
                return@setOnClickListener
            }
            val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            clipboard.setPrimaryClip(ClipData.newPlainText(rotulo, ip))
            Toast.makeText(this, "$rotulo copiado: $ip", Toast.LENGTH_SHORT).show()
        }
    }

    /**
     * IPv4 atual das interfaces de Wi-Fi (`wlan*`) e de tethering USB
     * (`rndis*`/`usb*`/`ncm*`). Qualquer outra (dados móveis etc.) é ignorada:
     * não serve de alvo pro daemon.
     */
    private fun enderecosIp(): Pair<String?, String?> {
        var wifi: String? = null
        var usb: String? = null
        try {
            for (iface in NetworkInterface.getNetworkInterfaces()) {
                if (!iface.isUp || iface.isLoopback) continue
                val ipv4 = iface.inetAddresses.asSequence()
                    .filterIsInstance<Inet4Address>()
                    .firstOrNull()?.hostAddress ?: continue
                val nome = iface.name
                when {
                    nome.startsWith("wlan") -> if (wifi == null) wifi = ipv4
                    nome.startsWith("rndis") || nome.startsWith("usb") || nome.startsWith("ncm") ->
                        if (usb == null) usb = ipv4
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "falha ao enumerar interfaces de rede", e)
        }
        return Pair(wifi, usb)
    }

    private fun mostrarSplash() {
        primeiroFrame.set(false)
        splash.visibility = View.VISIBLE
        uiHandler.removeCallbacks(atualizarIps)
        uiHandler.post(atualizarIps)
    }

    private fun esconderSplash() {
        splash.visibility = View.GONE
        uiHandler.removeCallbacks(atualizarIps)
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        this.holder = holder
        mostrarSplash()

        // O receiver UDP sobe uma vez só e fica no ar independentemente das
        // trocas de resolução: rebindar o socket a cada troca correria risco de
        // "address already in use" enquanto o anterior ainda fecha.
        val udpReceiver = RtpReceiver(nalQueue)
        udpReceiver.start()
        receiver = udpReceiver

        val controlServer = ControlServer(
            onResolution = { w, h -> runOnUiThread { configurarDecoder(w, h) } },
            onTimeout = {
                runOnUiThread {
                    if (decoder == null) {
                        Log.i(TAG, "sem canal de controle, usando fallback ${FALLBACK_WIDTH}x$FALLBACK_HEIGHT")
                        configurarDecoder(FALLBACK_WIDTH, FALLBACK_HEIGHT)
                    }
                }
            }
        )
        controlServer.start()
        control = controlServer
    }

    /**
     * Cria (ou recria) o decoder na resolução pedida. Chamado sempre na UI
     * thread, então não precisa de sincronização com surfaceDestroyed.
     */
    private fun configurarDecoder(width: Int, height: Int) {
        val holder = this.holder ?: return
        if (decoder != null && width == decoderWidth && height == decoderHeight) {
            Log.i(TAG, "resolução ${width}x$height inalterada, mantendo o decoder")
            return
        }

        liberarDecoder()

        // NALs da resolução anterior só produziriam lixo no decoder novo; o
        // daemon manda SPS/PPS a cada segundo (config-interval=1) e um keyframe
        // a cada GOP, então a imagem volta em ~1s.
        nalQueue.clear()

        try {
            val codec = MediaCodec.createDecoderByType("video/avc")
            val format = MediaFormat.createVideoFormat("video/avc", width, height)
            codec.configure(format, holder.surface, null, 0)
            codec.setCallback(object : MediaCodec.Callback() {
                override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
                    val nal = nalQueue.poll(200, TimeUnit.MILLISECONDS)
                    val buffer = codec.getInputBuffer(index) ?: return
                    if (nal == null) {
                        codec.queueInputBuffer(index, 0, 0, 0, 0)
                        return
                    }
                    buffer.clear()
                    buffer.put(nal)
                    codec.queueInputBuffer(index, 0, nal.size, System.nanoTime() / 1000, 0)
                }

                override fun onOutputBufferAvailable(
                    codec: MediaCodec,
                    index: Int,
                    info: MediaCodec.BufferInfo
                ) {
                    codec.releaseOutputBuffer(index, info.size > 0)
                    // Primeiro frame de vídeo na Surface: a tela de espera já
                    // cumpriu o papel dela.
                    if (info.size > 0 && primeiroFrame.compareAndSet(false, true)) {
                        runOnUiThread { esconderSplash() }
                    }
                }

                override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                    Log.e(TAG, "erro no decoder", e)
                }

                override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
                    Log.i(TAG, "formato de saída mudou: $format")
                }
            })
            codec.start()
            decoder = codec
            decoderWidth = width
            decoderHeight = height
            Log.i(TAG, "decoder configurado em ${width}x$height")
        } catch (e: Exception) {
            Log.e(TAG, "falha ao configurar o decoder em ${width}x$height", e)
        }
    }

    private fun liberarDecoder() {
        decoder?.let {
            try {
                it.stop()
            } catch (e: IllegalStateException) {
                Log.w(TAG, "decoder já estava parado", e)
            }
            it.release()
        }
        decoder = null
        decoderWidth = 0
        decoderHeight = 0
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        this.holder = null
        uiHandler.removeCallbacks(atualizarIps)
        control?.stopServing()
        control = null
        receiver?.stopReceiving()
        receiver = null
        liberarDecoder()
        nalQueue.clear()
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}
}

/**
 * Canal de controle: escuta conexões TCP curtas do daemon, cada uma trazendo a
 * resolução em que o vídeo será enviado.
 *
 * Formato: `FLM/1 <largura>x<altura>\n` (o prefixo de versão é opcional na
 * leitura, para tolerar um daemon mais simples). O daemon conecta, escreve uma
 * linha e fecha -- não há sessão persistente ainda.
 */
private class ControlServer(
    private val onResolution: (Int, Int) -> Unit,
    private val onTimeout: () -> Unit
) : Thread() {

    private val running = AtomicBoolean(true)
    private var server: ServerSocket? = null

    override fun run() {
        try {
            val s = ServerSocket(CONTROL_PORT)
            // Sem SO_REUSEADDR uma reabertura rápida do app pode falhar com o
            // socket anterior ainda em TIME_WAIT.
            s.reuseAddress = true
            s.soTimeout = CONTROL_WAIT_MS
            server = s
            Log.i(TAG, "canal de controle escutando na porta $CONTROL_PORT")

            var jaAvisouTimeout = false
            while (running.get()) {
                val client = try {
                    s.accept()
                } catch (_: java.net.SocketTimeoutException) {
                    // Só cai no fallback uma vez, enquanto nenhum daemon falou
                    // conosco; depois disso o timeout é só o loop respirando.
                    if (!jaAvisouTimeout) {
                        jaAvisouTimeout = true
                        onTimeout()
                    }
                    continue
                }
                jaAvisouTimeout = true
                atender(client)
            }
        } catch (e: Exception) {
            if (running.get()) Log.e(TAG, "erro no canal de controle", e)
        } finally {
            server?.close()
        }
    }

    private fun atender(client: Socket) {
        try {
            client.soTimeout = 2000
            val linha = client.getInputStream().bufferedReader().readLine()
            Log.i(TAG, "controle recebeu: $linha")
            val m = Regex("""(\d+)x(\d+)""").find(linha ?: "")
            if (m == null) {
                Log.w(TAG, "mensagem de controle não reconhecida: $linha")
                return
            }
            val w = m.groupValues[1].toIntOrNull() ?: return
            val h = m.groupValues[2].toIntOrNull() ?: return
            if (w in 2..8192 && h in 2..8192) {
                onResolution(w, h)
            } else {
                Log.w(TAG, "resolução fora de faixa: ${w}x$h")
            }
        } catch (e: Exception) {
            Log.w(TAG, "falha ao ler o canal de controle", e)
        } finally {
            try {
                client.close()
            } catch (_: Exception) {
            }
        }
    }

    fun stopServing() {
        running.set(false)
        try {
            server?.close()
        } catch (_: Exception) {
        }
        try {
            join(500)
        } catch (_: InterruptedException) {
        }
    }
}

/** Recebe datagramas RTP em background e empilha as NALs Annex-B reconstruídas. */
private class RtpReceiver(private val nalQueue: LinkedBlockingQueue<ByteArray>) : Thread() {

    private val running = AtomicBoolean(true)
    private var socket: DatagramSocket? = null

    override fun run() {
        val depacketizer = RtpH264Depacketizer()
        val buf = ByteArray(65536)
        try {
            val sock = DatagramSocket(UDP_PORT)
            sock.soTimeout = 500
            sock.receiveBufferSize = 1 shl 20 // 1MB, absorve rajadas de keyframe sem drop no kernel
            socket = sock
            Log.i(TAG, "escutando RTP/UDP na porta $UDP_PORT")

            val packet = DatagramPacket(buf, buf.size)
            while (running.get()) {
                // packet.length fica "preso" no tamanho do datagrama anterior depois de um
                // receive(); sem resetar aqui, um pacote pequeno (ex: SPS/PPS) seguido de um
                // fragmento FU-A grande trunca o fragmento silenciosamente e corrompe a NAL.
                packet.setLength(buf.size)
                try {
                    sock.receive(packet)
                } catch (_: java.net.SocketTimeoutException) {
                    continue
                }
                val nal = depacketizer.depacketize(packet.data, packet.length)
                if (nal != null) {
                    nalQueue.offer(nal)
                }
            }
        } catch (e: Exception) {
            if (running.get()) Log.e(TAG, "erro no receiver UDP", e)
        } finally {
            socket?.close()
        }
    }

    fun stopReceiving() {
        running.set(false)
        socket?.close()
        try {
            join(500)
        } catch (_: InterruptedException) {
        }
    }
}
