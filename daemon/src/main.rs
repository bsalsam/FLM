use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;

/// Fase 1: em vez de espelhar a região de um monitor físico (Fase 0 capturava
/// o eDP), o daemon localiza pelo nome um monitor RandR — idealmente o monitor
/// virtual criado para o Android — e captura só a região dele.
/// Ver docs/plano-arquitetura.md.
const UDP_PORT: u32 = 5000;

/// Nome padrão do monitor virtual. Vale tanto para um output RandR real
/// (caminho vkms/`--setprovideroutputsource`, em que o nome é o do output,
/// ex. "Virtual-1") quanto para uma região criada com
/// `xrandr --setmonitor FLM-0 ... none`: os dois aparecem em `GetMonitors`.
const DEFAULT_MONITOR: &str = "FLM-0";

/// Região da tela X11 (em pixels do screen combinado) ocupada por um monitor.
#[derive(Debug, Clone, Copy)]
struct Region {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

/// Descobre a geometria do monitor `name` via RandR 1.5 (`GetMonitors`).
fn find_monitor(name: &str) -> Result<Region> {
    let (conn, screen_num) = x11rb::connect(None).context("falha ao conectar no servidor X")?;
    let root = conn.setup().roots[screen_num].root;

    // get_active=false: inclui também monitores definidos por
    // `xrandr --setmonitor ... none`, que não têm nenhum output associado.
    let monitors = conn
        .randr_get_monitors(root, false)
        .context("falha ao pedir a lista de monitores RandR")?
        .reply()
        .context("o servidor X não respondeu GetMonitors (RandR 1.5 disponível?)")?;

    let mut encontrados = Vec::new();
    for m in monitors.monitors {
        let atom = conn.get_atom_name(m.name)?.reply()?;
        let m_name = String::from_utf8_lossy(&atom.name).into_owned();
        if m_name == name {
            return Ok(Region {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            });
        }
        encontrados.push(m_name);
    }

    anyhow::bail!(
        "monitor {name:?} não existe. Monitores disponíveis: {}. \
         Crie o monitor virtual antes de iniciar o daemon (ver docs/plano-arquitetura.md).",
        encontrados.join(", ")
    )
}

fn main() -> Result<()> {
    gst::init().context("falha ao inicializar o GStreamer")?;

    let mut args = std::env::args().skip(1);
    let udp_host = args
        .next()
        .context("uso: flm-daemon <ip-do-dispositivo-android> [nome-do-monitor]")?;
    let monitor_name = args.next().unwrap_or_else(|| DEFAULT_MONITOR.to_string());

    let region = find_monitor(&monitor_name)?;

    // H.264 exige dimensões pares; arredonda para baixo em vez de deixar o
    // x264enc falhar na negociação de caps se o monitor tiver largura/altura ímpar.
    let width = region.width & !1;
    let height = region.height & !1;
    anyhow::ensure!(
        width >= 2 && height >= 2,
        "monitor {monitor_name:?} tem geometria inválida para captura: {}x{}",
        region.width,
        region.height
    );

    // ximagesrc usa coordenadas inclusivas em endx/endy.
    let startx = region.x as i32;
    let starty = region.y as i32;
    let endx = startx + width as i32 - 1;
    let endy = starty + height as i32 - 1;

    // videoconvert força I420 (4:2:0) explicitamente: sem isso, o x264enc
    // herda o Y444 (4:4:4) nativo do ximagesrc e gera High 4:4:4 Predictive,
    // que decoders de hardware em celular (Qualcomm Venus etc.) não suportam
    // -- só decodificam 4:2:0. O decoder por software do GStreamer no PC
    // tolera 4:4:4, o que mascarou o problema no teste local.
    // threads=1: com tune=zerolatency, o x264enc ativa "sliced-threads" ao detectar
    // múltiplos núcleos (paraleliza sem adicionar latência de lookahead), gerando
    // frames com várias slices H.264. O decoder de hardware Qualcomm Venus do Motorola
    // G8 Power só decodifica a primeira slice corretamente e faz error concealment
    // (preenchimento magenta) no resto do frame -- mesma classe de bug do mismatch
    // 4:4:4/4:2:0 (invisível no decoder por software do PC, quebra só no hardware).
    // threads=1 força uma slice por frame e resolve; validado via teste local
    // (arquivo .h264 isolado, sem RTP/rede) no player padrão do Android.
    let pipeline_desc = format!(
        "ximagesrc use-damage=0 startx={startx} starty={starty} endx={endx} endy={endy} \
         ! video/x-raw,framerate=30/1 ! videoconvert ! video/x-raw,format=I420 \
         ! queue max-size-buffers=2 leaky=downstream \
         ! x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 threads=1 \
         ! rtph264pay config-interval=1 pt=96 \
         ! udpsink host={udp_host} port={UDP_PORT}"
    );

    let pipeline = gst::parse::launch(&pipeline_desc)
        .context("falha ao montar o pipeline de captura")?
        .downcast::<gst::Pipeline>()
        .expect("parse::launch com múltiplos elementos deve retornar uma Pipeline");

    pipeline
        .set_state(gst::State::Playing)
        .context("falha ao iniciar o pipeline")?;

    println!(
        "FLM daemon: capturando o monitor {monitor_name:?} ({width}x{height}+{startx}+{starty}) \
         e transmitindo para {udp_host}:{UDP_PORT} (Ctrl+C para parar)"
    );

    let bus = pipeline.bus().expect("pipeline sem bus");
    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;
        match msg.view() {
            MessageView::Eos(_) => {
                println!("fim de stream");
                break;
            }
            MessageView::Error(err) => {
                pipeline.set_state(gst::State::Null)?;
                anyhow::bail!(
                    "erro no elemento {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
            }
            _ => {}
        }
    }

    pipeline.set_state(gst::State::Null)?;
    Ok(())
}
