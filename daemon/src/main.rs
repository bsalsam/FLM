use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;

/// Fase 0 (POC): captura da tela inteira via XShm (ximagesrc), encode H.264
/// por software (zerolatency) e envio via RTP/UDP. Sem monitor virtual,
/// VAAPI ou canal de controle ainda — só validar pixels chegando no Android
/// com latência aceitável. Ver docs/plano-arquitetura.md.
const UDP_PORT: u32 = 5000;

fn main() -> Result<()> {
    gst::init().context("falha ao inicializar o GStreamer")?;

    let udp_host = std::env::args()
        .nth(1)
        .context("uso: flm-daemon <ip-do-dispositivo-android>")?;

    // Fase 0: captura só a região do monitor eDP (1920x1080), não a tela X11
    // combinada (3000x1920, os dois monitores lado a lado) -- o decoder HW
    // do dispositivo Android é configurado com essa mesma resolução fixa,
    // e decoders Qualcomm mais simples travam com um mismatch de resolução
    // grande em vez de renegociar o formato de saída.
    // videoconvert força I420 (4:2:0) explicitamente: sem isso, o x264enc
    // herda o Y444 (4:4:4) nativo do ximagesrc e gera High 4:4:4 Predictive,
    // que decoders de hardware em celular (Qualcomm Venus etc.) não suportam
    // -- só decodificam 4:2:0. O decoder por software do GStreamer no PC
    // tolera 4:4:4, o que mascarou o problema no teste local.
    let pipeline_desc = format!(
        "ximagesrc use-damage=0 startx=0 starty=840 endx=1919 endy=1919 \
         ! video/x-raw,framerate=30/1 ! videoconvert ! video/x-raw,format=I420 \
         ! queue max-size-buffers=2 leaky=downstream \
         ! x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 \
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

    println!("FLM daemon: transmitindo captura de tela para {udp_host}:{UDP_PORT} (Ctrl+C para parar)");

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
