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
             ! video/x-raw,framerate={framerate}/1 ! videoconvert ! video/x-raw,format=I420 \
             ! queue max-size-buffers=2 leaky=downstream \
             ! x264enc tune=zerolatency speed-preset=ultrafast key-int-max={framerate} threads=1 \
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
