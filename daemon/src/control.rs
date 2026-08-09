//! Canal de controle mínimo: informa a resolução ao cliente Android.
//!
//! # Por que isto existe
//!
//! O `MediaCodec` do Android era configurado com a resolução fixa em 1024x768,
//! porque o decoder de hardware Qualcomm não renegocia dimensões pelo SPS: com
//! mismatch ele simplesmente não produz imagem. Enquanto a resolução do monitor
//! virtual também era fixa isso funcionava, mas a bandeja agora deixa trocar o
//! modo -- então a resolução precisa atravessar para o outro lado.
//!
//! # Forma
//!
//! É a versão mínima do canal de controle TCP previsto na arquitetura (Fase 2/3
//! de `docs/plano-arquitetura.md`), não a definitiva: uma conexão curta, uma
//! linha de texto, fecha. Sem keepalive, sem pedido de keyframe, sem handshake
//! de duas vias -- só o suficiente para a troca de resolução funcionar ponta a
//! ponta. O daemon é o cliente TCP porque só ele sabe o endereço do outro lado
//! (não há mDNS ainda); o Android escuta.
//!
//! Formato: `FLM/1 <largura>x<altura>\n` em ASCII. O prefixo de versão existe
//! para que uma futura mensagem mais rica possa ser distinguida sem ambiguidade.

use anyhow::{Context, Result};
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Porta do canal de controle. Separada da porta de mídia (5000/UDP).
pub const CONTROL_PORT: u16 = 5001;

const TIMEOUT: Duration = Duration::from_secs(3);

/// Envia a resolução ao cliente Android e fecha a conexão.
///
/// Deve ser chamado *antes* de o pipeline começar a mandar RTP: o app precisa
/// criar o `MediaCodec` com as dimensões certas antes do primeiro pacote chegar.
pub fn send_resolution(host: &str, width: u16, height: u16) -> Result<()> {
    let addr = (host, CONTROL_PORT)
        .to_socket_addrs()
        .with_context(|| format!("endereço inválido: {host}"))?
        .next()
        .with_context(|| format!("nenhum endereço resolvido para {host}"))?;

    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)
        .with_context(|| format!("falha ao conectar em {addr} (o app Android está aberto?)"))?;
    stream.set_write_timeout(Some(TIMEOUT))?;

    stream
        .write_all(format!("FLM/1 {width}x{height}\n").as_bytes())
        .context("falha ao enviar a resolução")?;
    stream.flush().context("falha ao dar flush no canal de controle")?;
    Ok(())
}
