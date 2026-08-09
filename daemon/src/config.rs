//! Configuração persistente do daemon, em `~/.config/flm/config.toml`.
//!
//! Guarda o monitor virtual escolhido e os dois "alvos" de rede (Wi-Fi e USB).
//! Não existe descoberta automática (mDNS) ainda -- ver Fase 3 em
//! `docs/plano-arquitetura.md` --, então o IP do Android é digitado à mão e
//! preservado entre execuções. Ter dois campos separados evita ficar
//! redigitando o IP toda vez que se alterna entre Wi-Fi e cabo, já que o
//! tethering USB usa uma faixa completamente diferente da rede doméstica.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Qual dos dois alvos salvos está ativo.
///
/// Hoje isto é puramente "para qual IP eu mando o UDP": o transporte é o mesmo
/// (RTP/UDP) nos dois casos, porque o tethering USB aparece no Linux como mais
/// uma interface de rede. O rótulo existe para o usuário, não para o protocolo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TargetMode {
    #[default]
    Wifi,
    Usb,
}

impl TargetMode {
    pub fn label(self) -> &'static str {
        match self {
            TargetMode::Wifi => "Wi-Fi",
            TargetMode::Usb => "USB (tethering)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Nome do monitor RandR a capturar. O nome real depende de como o vkms foi
    /// anexado (costuma ser "Virtual-1-1"), por isso é descoberto em tempo de
    /// execução pelo menu da bandeja em vez de ficar fixo no código.
    pub monitor: String,
    /// Alvo ativo: para qual IP o `udpsink` vai mandar.
    pub mode: TargetMode,
    pub wifi_ip: String,
    pub usb_ip: String,
    /// Framerate de captura pedido ao `ximagesrc`.
    pub framerate: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            monitor: String::new(),
            mode: TargetMode::default(),
            wifi_ip: String::new(),
            usb_ip: String::new(),
            framerate: 30,
        }
    }
}

impl Config {
    /// IP do alvo ativo. Vazio significa "ainda não configurado".
    pub fn active_ip(&self) -> &str {
        match self.mode {
            TargetMode::Wifi => &self.wifi_ip,
            TargetMode::Usb => &self.usb_ip,
        }
    }

    pub fn path() -> Result<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .context("nem XDG_CONFIG_HOME nem HOME estão definidos")?;
        Ok(base.join("flm").join("config.toml"))
    }

    /// Carrega a configuração. Um arquivo ausente não é erro (primeira
    /// execução); um arquivo corrompido é, para não sobrescrever silenciosamente
    /// os IPs que o usuário digitou.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(texto) => {
                toml::from_str(&texto).with_context(|| format!("config inválida em {path:?}"))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("falha ao ler {path:?}")),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("falha ao criar o diretório {dir:?}"))?;
        }
        let texto = toml::to_string_pretty(self).context("falha ao serializar a config")?;
        std::fs::write(&path, texto).with_context(|| format!("falha ao escrever {path:?}"))
    }
}
