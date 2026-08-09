//! Descoberta de monitores e modos via RandR.
//!
//! Generaliza o antigo `find_monitor()`: a bandeja precisa *listar* os monitores
//! para o usuário escolher, não só localizar um pelo nome. A enumeração é feita
//! nativamente com `x11rb` (RandR 1.5 `GetMonitors` + 1.2 `GetScreenResources`),
//! mas a *troca* de modo é delegada ao binário `xrandr` -- ver `set_mode()`.

use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::ConnectionExt as _;

/// Região da tela X11 (em pixels do screen combinado) ocupada por um monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub name: String,
    pub region: Region,
    pub primary: bool,
    /// Outputs RandR associados. Vazio para monitores criados com
    /// `xrandr --setmonitor ... none`, que por isso não têm modo para trocar.
    pub outputs: Vec<String>,
}

impl MonitorInfo {
    /// Rótulo curto para o menu da bandeja.
    pub fn label(&self) -> String {
        format!(
            "{} ({}x{})",
            self.name, self.region.width, self.region.height
        )
    }

    /// Heurística para destacar candidatos a monitor virtual na UI. O output do
    /// vkms via `modesetting` sai como "Virtual-1-1" nesta máquina, mas o sufixo
    /// varia conforme a ordem em que o provider é anexado, então casa por prefixo.
    pub fn is_virtual(&self) -> bool {
        let n = self.name.to_ascii_lowercase();
        n.starts_with("virtual") || n.starts_with("flm-")
    }
}

/// Um modo de vídeo disponível para um output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u16,
    pub height: u16,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// Lista todos os monitores RandR do screen padrão.
pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
    let (conn, screen_num) = x11rb::connect(None).context("falha ao conectar no servidor X")?;
    let root = conn.setup().roots[screen_num].root;

    // get_active=false: inclui também monitores definidos por
    // `xrandr --setmonitor ... none`, que não têm nenhum output associado.
    let monitors = conn
        .randr_get_monitors(root, false)
        .context("falha ao pedir a lista de monitores RandR")?
        .reply()
        .context("o servidor X não respondeu GetMonitors (RandR 1.5 disponível?)")?;

    let mut resultado = Vec::new();
    for m in monitors.monitors {
        let atom = conn.get_atom_name(m.name)?.reply()?;
        let name = String::from_utf8_lossy(&atom.name).into_owned();

        let mut outputs = Vec::new();
        for out in &m.outputs {
            // Um output pode sumir entre GetMonitors e GetOutputInfo (hotplug);
            // nesse caso só o ignoramos em vez de derrubar a enumeração inteira.
            if let Ok(cookie) = conn.randr_get_output_info(*out, 0)
                && let Ok(info) = cookie.reply()
            {
                outputs.push(String::from_utf8_lossy(&info.name).into_owned());
            }
        }

        resultado.push(MonitorInfo {
            name,
            region: Region {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            },
            primary: m.primary,
            outputs,
        });
    }
    Ok(resultado)
}

/// Localiza a geometria do monitor `name`. Equivalente ao `find_monitor()`
/// original, agora escrito em cima da enumeração.
pub fn find_monitor(name: &str) -> Result<MonitorInfo> {
    let monitores = list_monitors()?;
    if let Some(m) = monitores.iter().find(|m| m.name == name) {
        return Ok(m.clone());
    }
    let nomes: Vec<_> = monitores.iter().map(|m| m.name.as_str()).collect();
    anyhow::bail!(
        "monitor {name:?} não existe. Monitores disponíveis: {}. \
         Crie o monitor virtual antes de iniciar o streaming (ver README.md).",
        if nomes.is_empty() {
            "(nenhum)".to_string()
        } else {
            nomes.join(", ")
        }
    )
}

/// Modos disponíveis para um output, deduplicados por resolução (ignoramos a
/// taxa de atualização: o framerate de captura é definido no pipeline, não pelo
/// modo do monitor virtual) e ordenados do maior para o menor.
pub fn list_modes(output_name: &str) -> Result<Vec<Mode>> {
    let (conn, screen_num) = x11rb::connect(None).context("falha ao conectar no servidor X")?;
    let root = conn.setup().roots[screen_num].root;

    let recursos = conn
        .randr_get_screen_resources(root)
        .context("falha ao pedir GetScreenResources")?
        .reply()
        .context("o servidor X não respondeu GetScreenResources")?;

    // Índice id-do-modo -> (largura, altura), montado a partir da tabela global
    // de modos do screen.
    let dims = |id: u32| -> Option<(u16, u16)> {
        recursos
            .modes
            .iter()
            .find(|mi| mi.id == id)
            .map(|mi| (mi.width, mi.height))
    };

    let mut modos: Vec<Mode> = Vec::new();
    for out in &recursos.outputs {
        let info = match conn.randr_get_output_info(*out, recursos.config_timestamp) {
            Ok(c) => match c.reply() {
                Ok(i) => i,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        if String::from_utf8_lossy(&info.name) != output_name {
            continue;
        }
        for id in &info.modes {
            if let Some((w, h)) = dims(*id) {
                let modo = Mode {
                    width: w,
                    height: h,
                };
                if !modos.contains(&modo) {
                    modos.push(modo);
                }
            }
        }
        // Ordena por área decrescente para o menu ficar previsível.
        modos.sort_by_key(|m| std::cmp::Reverse((m.width as u32) * (m.height as u32)));
        return Ok(modos);
    }

    anyhow::bail!("output RandR {output_name:?} não encontrado")
}

/// Aplica um modo a um output.
///
/// Deliberadamente delega ao binário `xrandr` em vez de usar `SetCrtcConfig` via
/// x11rb. Trocar o modo "na mão" pelo protocolo exige recalcular o tamanho do
/// screen, reposicionar os outros CRTCs e ajustar o DPI numa sequência com
/// ordem obrigatória (o screen precisa crescer antes e encolher depois); o
/// `xrandr` já faz exatamente isso e é a ferramenta que o usuário usaria
/// manualmente. Como a geometria é relida via RandR logo depois, um
/// reposicionamento feito pelo mutter não quebra a captura.
pub fn set_mode(output_name: &str, mode: Mode) -> Result<()> {
    let saida = std::process::Command::new("xrandr")
        .args(["--output", output_name, "--mode", &mode.to_string()])
        .output()
        .context("falha ao executar `xrandr` (o pacote x11-xserver-utils está instalado?)")?;

    anyhow::ensure!(
        saida.status.success(),
        "`xrandr --output {output_name} --mode {mode}` falhou: {}",
        String::from_utf8_lossy(&saida.stderr).trim()
    );
    Ok(())
}

/// Providers RandR, usados para anexar o vkms como saída da GPU principal.
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: randr::Provider,
    pub name: String,
    pub source_output: bool,
    pub sink_output: bool,
}

pub fn list_providers() -> Result<Vec<Provider>> {
    let (conn, screen_num) = x11rb::connect(None).context("falha ao conectar no servidor X")?;
    let root = conn.setup().roots[screen_num].root;

    let providers = conn
        .randr_get_providers(root)
        .context("falha ao pedir GetProviders")?
        .reply()
        .context("o servidor X não respondeu GetProviders")?;

    let mut resultado = Vec::new();
    for id in providers.providers {
        let info = match conn.randr_get_provider_info(id, providers.timestamp) {
            Ok(c) => match c.reply() {
                Ok(i) => i,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        resultado.push(Provider {
            id,
            name: String::from_utf8_lossy(&info.name).into_owned(),
            source_output: info
                .capabilities
                .contains(randr::ProviderCapability::SOURCE_OUTPUT),
            sink_output: info
                .capabilities
                .contains(randr::ProviderCapability::SINK_OUTPUT),
        });
    }
    Ok(resultado)
}

/// Anexa o provider do vkms (sink de saída) ao provider da GPU real (fonte de
/// saída), equivalente a `xrandr --setprovideroutputsource <sink> <source>`.
///
/// Precisa ser refeito a cada boot, depois do `modprobe vkms`. Não exige sudo.
pub fn attach_vkms_provider() -> Result<String> {
    let providers = list_providers()?;

    let sink = providers
        .iter()
        .find(|p| p.sink_output && !p.source_output)
        .context(
            "nenhum provider com capacidade de 'Sink Output' encontrado. \
             O módulo vkms está carregado? Rode `sudo modprobe vkms` (ver README.md).",
        )?;
    let source = providers
        .iter()
        .find(|p| p.source_output && p.id != sink.id)
        .context("nenhum provider com capacidade de 'Source Output' encontrado")?;

    // Os ids PRECISAM ir em hexadecimal com prefixo "0x": o xrandr interpreta um
    // número decimal solto como *índice* na lista de providers, não como XID
    // ("Could not find provider with index 1061"). Verificado na mão.
    let saida = std::process::Command::new("xrandr")
        .args([
            "--setprovideroutputsource",
            &format!("{:#x}", sink.id),
            &format!("{:#x}", source.id),
        ])
        .output()
        .context("falha ao executar `xrandr`")?;

    anyhow::ensure!(
        saida.status.success(),
        "`xrandr --setprovideroutputsource` falhou: {}",
        String::from_utf8_lossy(&saida.stderr).trim()
    );

    Ok(format!("{} anexado a {}", sink.name, source.name))
}
