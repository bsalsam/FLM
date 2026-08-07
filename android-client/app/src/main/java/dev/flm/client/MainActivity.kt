package dev.flm.client

import android.app.Activity
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Bundle
import android.util.Log
import android.view.SurfaceHolder
import android.view.SurfaceView
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

private const val TAG = "FlmClient"
private const val UDP_PORT = 5000

/**
 * POC da Fase 0: recebe RTP/H.264 vindo do daemon Linux e decodifica via
 * MediaCodec (modo assíncrono) direto numa Surface. Sem jitter buffer real,
 * sem mDNS/pareamento ainda -- só validar que os pixels chegam e são
 * decodificados. Ver docs/plano-arquitetura.md.
 */
class MainActivity : Activity(), SurfaceHolder.Callback {

    private lateinit var surfaceView: SurfaceView
    private var receiver: RtpReceiver? = null
    private var decoder: MediaCodec? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        surfaceView = findViewById(R.id.surface_view)
        surfaceView.holder.addCallback(this)
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        val nalQueue = LinkedBlockingQueue<ByteArray>()

        val codec = MediaCodec.createDecoderByType("video/avc")
        val format = MediaFormat.createVideoFormat("video/avc", 1920, 1080)
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

        val udpReceiver = RtpReceiver(nalQueue)
        udpReceiver.start()
        receiver = udpReceiver
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        receiver?.stopReceiving()
        receiver = null
        decoder?.let {
            it.stop()
            it.release()
        }
        decoder = null
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}
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
