//! Sessão de streaming: o pipeline GStreamer com ciclo de vida controlável.
//!
//! O daemon original rodava o pipeline até Ctrl+C no `main`. A bandeja precisa
//! poder iniciar e parar sob demanda, então o pipeline virou um objeto com
//! `start`/`stop` e uma thread separada vigiando o bus para reportar erros
//! assíncronos (que no modelo antigo simplesmente encerravam o processo).

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::randr::Region;

pub const UDP_PORT: u32 = 5000;

/// Escolhe o encoder H.264: VAAPI (hardware) quando a GPU oferece, senão x264
/// (software). Devolve o nome legível e o trecho do pipeline.
///
/// Restrições comuns aos dois, aprendidas a caro preço em decoders de hardware
/// de celular (Qualcomm Venus), que são bem menos tolerantes que o decode por
/// software do PC usado nos testes locais:
/// - chroma tem que ser 4:2:0 (NV12/I420) -- 4:4:4 herda do ximagesrc e o
///   decoder trava;
/// - UMA slice por frame -- com várias, o Venus decodifica só a primeira e
///   preenche o resto com error concealment (magenta). No x264 isso exige
///   `threads=1` (o tune=zerolatency liga sliced-threads em CPU multi-core);
///   no vah264enc o default já é `num-slices=1`, fixado aqui mesmo assim.
///
/// A troca pra VAAPI foi validada pelo mesmo método do bug das slices: arquivo
/// `.h264` local (sem rede/RTP), inspecionado (High 4:2:0, 1 slice/frame) e
/// tocado no player padrão do Android com decode limpo.
///
/// rate-control=vbr, não cbr: em CBR o encoder emite NALs de filler (tipo 12)
/// pra sustentar o bitrate quando o desktop está parado -- banda desperdiçada
/// (95 fillers em 6s medidos nesta GPU) e frames "vazios" que faziam o decoder
/// do celular piscar com artefatos. O depacketizer do app também descarta
/// filler por robustez, mas VBR nem gera.
fn trecho_encoder(framerate: u32) -> (&'static str, String) {
    if gst::ElementFactory::find("vah264enc").is_some() {
        // vapostproc faz upload + conversão BGRx→NV12 na GPU e entrega a
        // surface VA direto pro encoder (zero-copy): metade da CPU da variante
        // com videoconvert (20%→10% medidos; o que resta é o próprio ximagesrc).
        (
            "vah264enc (hardware VAAPI)",
            format!(
                "vapostproc ! video/x-raw(memory:VAMemory),format=NV12 \
                 ! queue max-size-buffers=2 leaky=downstream \
                 ! vah264enc rate-control=vbr bitrate=8000 key-int-max={framerate} \
                   num-slices=1 b-frames=0"
            ),
        )
    } else {
        (
            "x264enc (software)",
            format!(
                "videoconvert ! video/x-raw,format=I420 \
                 ! queue max-size-buffers=2 leaky=downstream \
                 ! x264enc tune=zerolatency speed-preset=ultrafast key-int-max={framerate} threads=1"
            ),
        )
    }
}

/// Erro assíncrono reportado pelo bus do GStreamer depois do start.
pub type ErrorSlot = Arc<Mutex<Option<String>>>;

pub struct Session {
    pipeline: gst::Pipeline,
    parando: Arc<AtomicBool>,
    vigia: Option<std::thread::JoinHandle<()>>,
    pub host: String,
    pub monitor: String,
    pub width: u16,
    pub height: u16,
    /// Nome legível do encoder em uso (VAAPI ou x264), pro status da bandeja.
    pub encoder: &'static str,
    /// Preenchido pela thread do bus se o pipeline morrer sozinho.
    pub erro: ErrorSlot,
}

impl Session {
    /// Monta e inicia o pipeline capturando `region` e enviando para `host`.
    pub fn start(
        host: &str,
        monitor: &str,
        region: Region,
        framerate: u32,
    ) -> Result<Self> {
        // H.264 exige dimensões pares; arredonda para baixo em vez de deixar o
        // x264enc falhar na negociação de caps se o monitor tiver largura/altura ímpar.
        let width = region.width & !1;
        let height = region.height & !1;
        anyhow::ensure!(
            width >= 2 && height >= 2,
            "monitor {monitor:?} tem geometria inválida para captura: {}x{}",
            region.width,
            region.height
        );

        // ximagesrc usa coordenadas inclusivas em endx/endy.
        let startx = region.x as i32;
        let starty = region.y as i32;
        let endx = startx + width as i32 - 1;
        let endy = starty + height as i32 - 1;

        let (encoder, trecho_encoder) = trecho_encoder(framerate);

        let pipeline_desc = format!(
            "ximagesrc use-damage=0 startx={startx} starty={starty} endx={endx} endy={endy} \
             ! video/x-raw,framerate={framerate}/1 \
             ! {trecho_encoder} \
             ! rtph264pay config-interval=1 pt=96 \
             ! udpsink host={host} port={UDP_PORT}"
        );

        let pipeline = gst::parse::launch(&pipeline_desc)
            .context("falha ao montar o pipeline de captura")?
            .downcast::<gst::Pipeline>()
            .expect("parse::launch com múltiplos elementos deve retornar uma Pipeline");

        pipeline
            .set_state(gst::State::Playing)
            .context("falha ao iniciar o pipeline")?;

        let erro: ErrorSlot = Arc::new(Mutex::new(None));
        let parando = Arc::new(AtomicBool::new(false));

        let vigia = {
            let bus = pipeline.bus().expect("pipeline sem bus");
            let erro = Arc::clone(&erro);
            let parando = Arc::clone(&parando);
            let pipeline = pipeline.clone();
            std::thread::spawn(move || {
                // timed_pop com timeout curto em vez de iter_timed(NONE): assim a
                // thread acorda periodicamente e percebe o pedido de parada, em
                // vez de ficar presa para sempre num bus que nunca posta nada.
                while !parando.load(Ordering::Relaxed) {
                    let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else {
                        continue;
                    };
                    match msg.view() {
                        gst::MessageView::Eos(_) => {
                            *erro.lock().unwrap() = Some("fim de stream inesperado".into());
                            break;
                        }
                        gst::MessageView::Error(e) => {
                            *erro.lock().unwrap() = Some(format!(
                                "erro no elemento {:?}: {} ({:?})",
                                e.src().map(|s| s.path_string()),
                                e.error(),
                                e.debug()
                            ));
                            let _ = pipeline.set_state(gst::State::Null);
                            break;
                        }
                        _ => {}
                    }
                }
            })
        };

        Ok(Session {
            pipeline,
            parando,
            vigia: Some(vigia),
            host: host.to_string(),
            monitor: monitor.to_string(),
            width,
            height,
            encoder,
            erro,
        })
    }

    /// Erro assíncrono, se o pipeline tiver morrido sozinho.
    pub fn erro_atual(&self) -> Option<String> {
        self.erro.lock().unwrap().clone()
    }

    pub fn stop(mut self) {
        self.parando.store(true, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(v) = self.vigia.take() {
            let _ = v.join();
        }
    }
}
