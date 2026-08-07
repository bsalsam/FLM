use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;

/// Fase 0 (POC): captura da tela inteira via XShm (ximagesrc), encode H.264
/// por software (zerolatency) e envio via RTP/UDP. Sem monitor virtual,
/// VAAPI ou canal de controle ainda — só validar pixels chegando no Android
/// com latência aceitável. Ver docs/plano-arquitetura.md.
const UDP_HOST: &str = "0.0.0.0";
const UDP_PORT: u32 = 5000;

fn main() -> Result<()> {
    gst::init().context("falha ao inicializar o GStreamer")?;

    let pipeline_desc = format!(
        "ximagesrc use-damage=0 ! video/x-raw,framerate=30/1 ! videoconvert \
         ! queue max-size-buffers=2 leaky=downstream \
         ! x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 \
         ! rtph264pay config-interval=1 pt=96 \
         ! udpsink host={UDP_HOST} port={UDP_PORT}"
    );

    let pipeline = gst::parse::launch(&pipeline_desc)
        .context("falha ao montar o pipeline de captura")?
        .downcast::<gst::Pipeline>()
        .expect("parse::launch com múltiplos elementos deve retornar uma Pipeline");

    pipeline
        .set_state(gst::State::Playing)
        .context("falha ao iniciar o pipeline")?;

    println!("FLM daemon: transmitindo captura de tela via UDP porta {UDP_PORT} (Ctrl+C para parar)");

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
