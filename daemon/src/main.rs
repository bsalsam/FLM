//! FLM daemon — captura um monitor virtual X11 e transmite via RTP/H.264.
//!
//! Dois modos de uso:
//!
//! - **bandeja** (padrão, sem argumentos): ícone StatusNotifierItem com
//!   iniciar/parar, escolha de monitor, resolução e alvo de rede. Ver `tray.rs`.
//! - **one-shot** (`flm-daemon <ip> [monitor]`): o comportamento original,
//!   preservado porque é o caminho mais direto para depurar o pipeline sem
//!   depender de D-Bus, da bandeja ou da extensão do GNOME Shell.
//!
//! Ver docs/plano-arquitetura.md e README.md.

mod config;
mod control;
mod dialog;
mod randr;
mod stream;
mod tray;

use anyhow::{Context, Result};
use gstreamer as gst;
use ksni::blocking::TrayMethods;

use config::Config;
use stream::Session;

const AJUDA: &str = "\
FLM daemon — Free Linux Monitor on Android

USO:
    flm-daemon                        inicia o ícone de bandeja (padrão)
    flm-daemon --tray                 idem, explícito
    flm-daemon <ip> [monitor]         modo one-shot: transmite até Ctrl+C
    flm-daemon --list-monitors        lista os monitores RandR e sai
    flm-daemon --help                 esta ajuda

O modo one-shot ignora o arquivo de configuração e não usa o canal de controle
de resolução; serve para depurar o pipeline isoladamente.
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{AJUDA}");
            Ok(())
        }
        Some("--list-monitors") => listar_monitores(),
        None | Some("--tray") => rodar_bandeja(),
        Some(ip) => rodar_one_shot(ip, args.get(1).cloned()),
    }
}

fn listar_monitores() -> Result<()> {
    for m in randr::list_monitors()? {
        let marca = if m.primary { "*" } else { " " };
        let outputs = if m.outputs.is_empty() {
            "(sem output)".to_string()
        } else {
            m.outputs.join(",")
        };
        println!(
            "{marca} {:<16} {}x{}+{}+{}  outputs: {outputs}",
            m.name, m.region.width, m.region.height, m.region.x, m.region.y
        );
    }
    Ok(())
}

/// Garante uma única instância da bandeja: tranca (flock) um arquivo em
/// `XDG_RUNTIME_DIR`. O kernel solta o lock quando o processo morre, inclusive
/// em crash, então nunca há trava velha a limpar. Devolve `None` se outra
/// instância já segura a trava. O modo one-shot não passa por aqui de
/// propósito: é ferramenta de depuração e pode coexistir com a bandeja.
fn trava_instancia_unica() -> Result<Option<std::fs::File>> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let arquivo = std::fs::File::create(dir.join("flm-daemon.lock"))
        .context("falha ao criar o arquivo de trava de instância única")?;
    match arquivo.try_lock() {
        Ok(()) => Ok(Some(arquivo)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).context("falha ao trancar o arquivo de instância única")
        }
    }
}

fn rodar_bandeja() -> Result<()> {
    // A trava precisa viver até o fim do processo — soltá-la liberaria o flock.
    let Some(_trava) = trava_instancia_unica()? else {
        // Lançado pelo menu do sistema não há terminal visível; avisa por
        // notificação de desktop além do stderr.
        let _ = std::process::Command::new("notify-send")
            .args([
                "--app-name=FLM",
                "FLM já está rodando",
                "O ícone já está na bandeja do sistema.",
            ])
            .status();
        eprintln!("FLM já está rodando (ícone na bandeja); nada a fazer.");
        return Ok(());
    };

    gst::init().context("falha ao inicializar o GStreamer")?;

    let cfg = Config::load().unwrap_or_else(|e| {
        eprintln!("aviso: {e:#} — começando com a configuração padrão");
        Config::default()
    });

    let handle = tray::nova(cfg).spawn().context(
        "falha ao registrar o ícone na bandeja. \
         O GNOME Shell precisa da extensão AppIndicator habilitada — ver README.md.",
    )?;

    // O tray precisa de um handle para si mesmo para poder receber o resultado
    // das operações que rodam fora do loop de serviço.
    let copia = handle.clone();
    handle.update(move |t| t.set_handle(copia));

    println!("FLM: ícone de bandeja ativo. Use o menu para iniciar o streaming.");

    // O trabalho todo acontece nas threads do ksni e nas que ele dispara.
    loop {
        std::thread::park();
    }
}

fn rodar_one_shot(ip: &str, monitor: Option<String>) -> Result<()> {
    gst::init().context("falha ao inicializar o GStreamer")?;

    let monitor = monitor.unwrap_or_else(|| "FLM-0".to_string());
    let info = randr::find_monitor(&monitor)?;

    let sessao = Session::start(ip, &monitor, info.region, 30)?;
    println!(
        "FLM daemon: capturando o monitor {monitor:?} ({}x{}+{}+{}) via {} \
         e transmitindo para {ip}:{} (Ctrl+C para parar)",
        sessao.width,
        sessao.height,
        info.region.x,
        info.region.y,
        sessao.encoder,
        stream::UDP_PORT
    );

    // Espera o pipeline morrer (erro/EOS) — Ctrl+C encerra o processo.
    loop {
        if let Some(e) = sessao.erro_atual() {
            anyhow::bail!("{e}");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
