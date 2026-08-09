//! Ícone de bandeja (StatusNotifierItem) com o controle do daemon.
//!
//! Usa `ksni`, que implementa o protocolo StatusNotifierItem falando D-Bus
//! diretamente -- Rust puro, sem depender de headers de GTK nem da libayatana.
//! Ver README.md ("Bandeja do sistema") para a nota sobre a extensão do GNOME
//! Shell que é obrigatória para o ícone aparecer.
//!
//! ## Modelo de concorrência
//!
//! Todo o estado mora dentro da própria struct `FlmTray`; o `ksni` serializa o
//! acesso a ele no seu loop de serviço, então os callbacks de menu recebem
//! `&mut self` sem `Mutex` nenhum. Em compensação, **os callbacks não podem
//! bloquear** -- se bloquearem, o menu congela. Por isso toda operação lenta
//! (diálogo `zenity`, `xrandr`, conectar TCP, montar o pipeline) roda numa
//! thread separada, que devolve o resultado com `Handle::update`.

use ksni::menu::*;
use ksni::{MenuItem, Tray};

use crate::config::{Config, TargetMode};
use crate::randr::{self, Mode, MonitorInfo};
use crate::stream::{self, Session};
use crate::{control, dialog};

type Handle = ksni::blocking::Handle<FlmTray>;

/// Constrói a bandeja já com a enumeração RandR feita, para que o tooltip e o
/// menu estejam corretos antes mesmo da primeira abertura.
pub fn nova(config: Config) -> FlmTray {
    let mut t = FlmTray::new(config);
    t.atualizar_randr();
    t
}

pub struct FlmTray {
    config: Config,
    session: Option<Session>,
    /// Cache da enumeração RandR, atualizado ao abrir o menu.
    monitores: Vec<MonitorInfo>,
    modos: Vec<Mode>,
    status: String,
    /// Evita disparar duas operações longas ao mesmo tempo (ex.: clique duplo
    /// em "Iniciar" criando dois pipelines para o mesmo destino).
    ocupado: bool,
    handle: Option<Handle>,
}

impl FlmTray {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            session: None,
            monitores: Vec::new(),
            modos: Vec::new(),
            status: "Parado".into(),
            ocupado: false,
            handle: None,
        }
    }

    pub fn set_handle(&mut self, h: Handle) {
        self.handle = Some(h);
    }

    /// Roda `f` numa thread, com um handle para escrever o resultado de volta.
    fn na_thread<F>(&self, f: F)
    where
        F: FnOnce(Handle) + Send + 'static,
    {
        if let Some(h) = self.handle.clone() {
            std::thread::spawn(move || f(h));
        }
    }

    fn transmitindo(&self) -> bool {
        self.session.is_some()
    }

    fn salvar_config(&mut self) {
        if let Err(e) = self.config.save() {
            self.status = format!("Falha ao salvar a config: {e}");
        }
    }

    /// Relê monitores e modos. Barato o bastante para rodar a cada abertura de
    /// menu, e assim o vkms anexado depois que a bandeja subiu já aparece.
    fn atualizar_randr(&mut self) {
        match randr::list_monitors() {
            Ok(ms) => {
                // Primeira execução: se ainda não há monitor escolhido, chuta o
                // primeiro que pareça virtual -- é quase sempre o certo, e evita
                // que o usuário precise configurar antes de conseguir testar.
                if self.config.monitor.is_empty()
                    && let Some(v) = ms.iter().find(|m| m.is_virtual())
                {
                    self.config.monitor = v.name.clone();
                    let _ = self.config.save();
                }
                self.monitores = ms;
            }
            Err(e) => {
                self.monitores.clear();
                self.status = format!("RandR indisponível: {e}");
            }
        }

        self.modos = self
            .monitor_atual()
            .and_then(|m| m.outputs.first().cloned())
            .and_then(|out| randr::list_modes(&out).ok())
            .unwrap_or_default();
    }

    fn monitor_atual(&self) -> Option<&MonitorInfo> {
        self.monitores
            .iter()
            .find(|m| m.name == self.config.monitor)
    }

    /// Verifica se o pipeline morreu sozinho (erro no bus) e reflete no status.
    fn checar_saude(&mut self) {
        if let Some(s) = &self.session
            && let Some(e) = s.erro_atual()
        {
            self.status = format!("Pipeline caiu: {e}");
            if let Some(s) = self.session.take() {
                s.stop();
            }
        }
    }

    // ---- Ações ----

    fn acao_iniciar(&mut self) {
        if self.ocupado || self.transmitindo() {
            return;
        }
        let ip = self.config.active_ip().to_string();
        if ip.is_empty() {
            let modo = self.config.mode.label();
            self.status = format!("Defina o IP de {modo} antes de iniciar");
            let msg = format!(
                "Nenhum IP configurado para {modo}.\n\n\
                 Use o menu Conexão → \"Definir IP\" para informar o endereço do dispositivo Android."
            );
            self.na_thread(move |_| dialog::erro(&msg));
            return;
        }
        if self.config.monitor.is_empty() {
            self.status = "Escolha o monitor virtual antes de iniciar".into();
            self.na_thread(|_| {
                dialog::erro(
                    "Nenhum monitor virtual selecionado.\n\n\
                     Use o menu \"Monitor virtual\" para escolher (normalmente Virtual-1-1).\n\
                     Se a lista estiver vazia, carregue o vkms: sudo modprobe vkms",
                )
            });
            return;
        }

        self.ocupado = true;
        self.status = "Iniciando…".into();
        let monitor = self.config.monitor.clone();
        let framerate = self.config.framerate;

        self.na_thread(move |h| {
            let resultado = iniciar_sessao(&ip, &monitor, framerate);
            let msg_erro = h.update(move |t| {
                t.ocupado = false;
                match resultado {
                    Ok((sessao, aviso)) => {
                        t.status = match &aviso {
                            Some(a) => format!("Transmitindo (aviso: {a})"),
                            None => "Transmitindo".into(),
                        };
                        t.session = Some(sessao);
                        aviso.map(|a| {
                            format!(
                                "O streaming começou, mas o canal de controle falhou:\n\n{a}\n\n\
                                 O app Android vai continuar usando a resolução que já tinha. \
                                 Se a imagem não aparecer, abra o app e reinicie o streaming."
                            )
                        })
                    }
                    Err(e) => {
                        t.status = format!("Erro ao iniciar: {e}");
                        Some(format!("Não foi possível iniciar o streaming:\n\n{e}"))
                    }
                }
            });
            // Diálogo fora do update: dentro dele o loop do ksni ficaria travado
            // enquanto a janela do zenity estivesse aberta.
            if let Some(Some(m)) = msg_erro {
                dialog::erro(&m);
            }
        });
    }

    fn acao_parar(&mut self) {
        if let Some(s) = self.session.take() {
            // stop() faz join na thread do bus, que acorda a cada 200ms.
            self.na_thread(move |h| {
                s.stop();
                h.update(|t| t.status = "Parado".into());
            });
            self.status = "Parando…".into();
        }
    }

    fn acao_escolher_monitor(&mut self, idx: usize) {
        let Some((nome, output)) = self
            .monitores
            .get(idx)
            .map(|m| (m.name.clone(), m.outputs.first().cloned()))
        else {
            return;
        };
        if nome == self.config.monitor {
            return;
        }
        self.config.monitor = nome;
        self.salvar_config();
        self.modos = output
            .and_then(|out| randr::list_modes(&out).ok())
            .unwrap_or_default();
        if self.transmitindo() {
            self.status = "Monitor alterado — reinicie o streaming".into();
        }
    }

    fn acao_escolher_modo(&mut self, idx: usize) {
        let Some(modo) = self.modos.get(idx).copied() else {
            return;
        };
        let Some(output) = self
            .monitor_atual()
            .and_then(|m| m.outputs.first().cloned())
        else {
            self.status = "Este monitor não tem output RandR para trocar de modo".into();
            return;
        };
        if self.ocupado {
            return;
        }

        // Trocar a resolução com o streaming ligado exige o ciclo completo:
        // parar o pipeline, aplicar o modo, reavisar o Android da nova
        // resolução (o MediaCodec não renegocia dimensões) e subir de novo.
        let estava_rodando = self.transmitindo();
        let sessao = self.session.take();
        let ip = self.config.active_ip().to_string();
        let monitor = self.config.monitor.clone();
        let framerate = self.config.framerate;

        self.ocupado = true;
        self.status = format!("Aplicando {modo}…");

        self.na_thread(move |h| {
            if let Some(s) = sessao {
                s.stop();
            }
            let mut erro = randr::set_mode(&output, modo).err().map(|e| e.to_string());

            let mut nova_sessao = None;
            if erro.is_none() && estava_rodando {
                match iniciar_sessao(&ip, &monitor, framerate) {
                    Ok((s, aviso)) => {
                        nova_sessao = Some(s);
                        erro = aviso;
                    }
                    Err(e) => erro = Some(e.to_string()),
                }
            }

            let msg = h.update(move |t| {
                t.ocupado = false;
                t.session = nova_sessao;
                match &erro {
                    None => {
                        t.status = if t.transmitindo() {
                            "Transmitindo".into()
                        } else {
                            format!("Modo {modo} aplicado")
                        }
                    }
                    Some(e) => t.status = format!("Falha ao aplicar {modo}: {e}"),
                }
                erro.map(|e| format!("Problema ao trocar a resolução para {modo}:\n\n{e}"))
            });
            if let Some(Some(m)) = msg {
                dialog::erro(&m);
            }
        });
    }

    fn acao_definir_ip(&mut self, alvo: TargetMode) {
        let atual = match alvo {
            TargetMode::Wifi => self.config.wifi_ip.clone(),
            TargetMode::Usb => self.config.usb_ip.clone(),
        };
        let titulo = format!("FLM — IP via {}", alvo.label());
        let texto = match alvo {
            TargetMode::Wifi => {
                "Endereço IP do dispositivo Android na rede Wi-Fi.\n\
                 Veja em Configurações → Sobre o telefone → Status."
            }
            TargetMode::Usb => {
                "Endereço IP do dispositivo Android via tethering USB.\n\
                 Costuma ser algo como 192.168.x.1 — confira com `ip addr` na interface usb0."
            }
        };
        self.na_thread(move |h| {
            let Some(novo) = dialog::entrada(&titulo, texto, &atual) else {
                return;
            };
            h.update(move |t| {
                match alvo {
                    TargetMode::Wifi => t.config.wifi_ip = novo.clone(),
                    TargetMode::Usb => t.config.usb_ip = novo.clone(),
                }
                t.salvar_config();
                if t.config.mode == alvo {
                    t.status = if t.transmitindo() {
                        "IP alterado — reinicie o streaming".into()
                    } else {
                        format!("IP de {} definido: {novo}", alvo.label())
                    };
                }
            });
        });
    }

    fn acao_anexar_vkms(&mut self) {
        if self.ocupado {
            return;
        }
        self.ocupado = true;
        self.status = "Anexando provider vkms…".into();
        self.na_thread(|h| {
            let r = randr::attach_vkms_provider();
            let msg = h.update(move |t| {
                t.ocupado = false;
                match r {
                    Ok(desc) => {
                        t.atualizar_randr();
                        t.status = format!("Provider anexado: {desc}");
                        None
                    }
                    Err(e) => {
                        t.status = format!("Falha ao anexar o provider: {e}");
                        Some(format!("Não foi possível anexar o provider do vkms:\n\n{e}"))
                    }
                }
            });
            if let Some(Some(m)) = msg {
                dialog::erro(&m);
            }
        });
    }

    fn acao_sobre(&self) {
        let ip_wifi = se_vazio(&self.config.wifi_ip);
        let ip_usb = se_vazio(&self.config.usb_ip);
        let monitor = se_vazio(&self.config.monitor);
        let cfg = Config::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(desconhecido)".into());
        let texto = format!(
            "FLM — Free Linux Monitor on Android\nv{}\n\n\
             Monitor virtual: {monitor}\n\
             IP Wi-Fi: {ip_wifi}\nIP USB: {ip_usb}\n\n\
             Mídia: RTP/H.264 em UDP {}\n\
             Controle: TCP {} (envia a resolução ao app)\n\n\
             Configuração: {cfg}\n\n\
             Licença GPLv3.",
            env!("CARGO_PKG_VERSION"),
            stream::UDP_PORT,
            control::CONTROL_PORT,
        );
        self.na_thread(move |_| dialog::info(&texto));
    }
}

fn se_vazio(s: &str) -> &str {
    if s.is_empty() { "(não definido)" } else { s }
}

/// Sequência completa de início: localizar o monitor, avisar a resolução ao
/// Android e só então subir o pipeline.
///
/// Devolve a sessão e, opcionalmente, um aviso não-fatal (falha no canal de
/// controle). A falha do canal de controle é deliberadamente não-fatal: um APK
/// antigo, sem o servidor de controle, continua funcionando desde que a
/// resolução do monitor virtual seja a que ele espera.
fn iniciar_sessao(
    ip: &str,
    monitor: &str,
    framerate: u32,
) -> anyhow::Result<(Session, Option<String>)> {
    let info = randr::find_monitor(monitor)?;
    let region = info.region;
    let width = region.width & !1;
    let height = region.height & !1;

    let aviso = control::send_resolution(ip, width, height)
        .err()
        .map(|e| format!("{e:#}"));

    let sessao = Session::start(ip, monitor, region, framerate)?;
    Ok((sessao, aviso))
}

impl Tray for FlmTray {
    fn id(&self) -> String {
        "flm-daemon".into()
    }

    fn title(&self) -> String {
        "FLM — Free Linux Monitor".into()
    }

    fn icon_name(&self) -> String {
        "video-display".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn status(&self) -> ksni::Status {
        // Active deixa o ícone destacado enquanto está transmitindo, em painéis
        // que diferenciam os dois estados.
        if self.transmitindo() {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let descricao = match &self.session {
            Some(s) => format!(
                "Transmitindo {} ({}x{}) para {}",
                s.monitor, s.width, s.height, s.host
            ),
            None => self.status.clone(),
        };
        ksni::ToolTip {
            icon_name: "video-display".into(),
            title: "FLM".into(),
            description: descricao,
            icon_pixmap: Vec::new(),
        }
    }

    /// Recarrega o estado logo antes de o menu ser desenhado, para o usuário
    /// nunca ver uma lista de monitores obsoleta nem um "Transmitindo" que já
    /// morreu.
    fn menu_about_to_show(&mut self) {
        self.checar_saude();
        self.atualizar_randr();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut itens: Vec<MenuItem<Self>> = Vec::new();

        // --- Status (linhas informativas, não clicáveis) ---
        // Com o streaming ligado, mostra o que a *sessão* está de fato usando,
        // não o que está na config: os dois divergem se o usuário editar o IP
        // ou trocar de monitor sem reiniciar.
        let linha_status = match &self.session {
            Some(s) => format!("● Transmitindo para {}", s.host),
            None => format!("○ {}", self.status),
        };
        itens.push(
            StandardItem {
                label: linha_status,
                enabled: false,
                ..Default::default()
            }
            .into(),
        );

        let detalhe = match (&self.session, self.monitor_atual()) {
            (Some(s), _) => format!("   Monitor: {} — {}x{}", s.monitor, s.width, s.height),
            (None, Some(m)) => format!(
                "   Monitor: {} — {}x{}",
                m.name, m.region.width, m.region.height
            ),
            (None, None) if self.config.monitor.is_empty() => {
                "   Monitor: (não escolhido)".to_string()
            }
            (None, None) => format!("   Monitor: {} (ausente!)", self.config.monitor),
        };
        itens.push(
            StandardItem {
                label: detalhe,
                enabled: false,
                ..Default::default()
            }
            .into(),
        );
        itens.push(
            StandardItem {
                label: format!("   Conexão: {}", self.config.mode.label()),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );
        itens.push(MenuItem::Separator);

        // --- Iniciar / Parar ---
        if self.transmitindo() {
            itens.push(
                StandardItem {
                    label: "Parar streaming".into(),
                    icon_name: "media-playback-stop".into(),
                    enabled: !self.ocupado,
                    activate: Box::new(|t: &mut Self| t.acao_parar()),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            itens.push(
                StandardItem {
                    label: "Iniciar streaming".into(),
                    icon_name: "media-playback-start".into(),
                    enabled: !self.ocupado,
                    activate: Box::new(|t: &mut Self| t.acao_iniciar()),
                    ..Default::default()
                }
                .into(),
            );
        }
        itens.push(MenuItem::Separator);

        // --- Monitor virtual ---
        let sel_monitor = self
            .monitores
            .iter()
            .position(|m| m.name == self.config.monitor)
            .unwrap_or(usize::MAX);
        let mut submenu_monitor: Vec<MenuItem<Self>> = Vec::new();
        if self.monitores.is_empty() {
            submenu_monitor.push(
                StandardItem {
                    label: "(nenhum monitor detectado)".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else {
            submenu_monitor.push(
                RadioGroup {
                    selected: sel_monitor,
                    select: Box::new(|t: &mut Self, i| t.acao_escolher_monitor(i)),
                    options: self
                        .monitores
                        .iter()
                        .map(|m| RadioItem {
                            label: m.label(),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into(),
            );
        }
        submenu_monitor.push(MenuItem::Separator);
        submenu_monitor.push(
            StandardItem {
                label: "Anexar provider vkms".into(),
                enabled: !self.ocupado,
                activate: Box::new(|t: &mut Self| t.acao_anexar_vkms()),
                ..Default::default()
            }
            .into(),
        );
        itens.push(
            SubMenu {
                label: "Monitor virtual".into(),
                submenu: submenu_monitor,
                ..Default::default()
            }
            .into(),
        );

        // --- Resolução ---
        let atual = self.monitor_atual().map(|m| m.region);
        let sel_modo = atual
            .and_then(|r| {
                self.modos
                    .iter()
                    .position(|m| m.width == r.width && m.height == r.height)
            })
            .unwrap_or(usize::MAX);
        let submenu_modo: Vec<MenuItem<Self>> = if self.modos.is_empty() {
            vec![
                StandardItem {
                    label: "(sem modos — monitor sem output RandR)".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            ]
        } else {
            vec![
                RadioGroup {
                    selected: sel_modo,
                    select: Box::new(|t: &mut Self, i| t.acao_escolher_modo(i)),
                    options: self
                        .modos
                        .iter()
                        .map(|m| RadioItem {
                            label: m.to_string(),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into(),
            ]
        };
        itens.push(
            SubMenu {
                label: "Resolução".into(),
                enabled: !self.ocupado,
                submenu: submenu_modo,
                ..Default::default()
            }
            .into(),
        );

        // --- Conexão (Wi-Fi / USB) ---
        let modos_conexao = [TargetMode::Wifi, TargetMode::Usb];
        let sel_conexao = modos_conexao
            .iter()
            .position(|m| *m == self.config.mode)
            .unwrap_or(0);
        let submenu_conexao: Vec<MenuItem<Self>> = vec![
            RadioGroup {
                selected: sel_conexao,
                select: Box::new(|t: &mut Self, i| {
                    let novo = if i == 0 {
                        TargetMode::Wifi
                    } else {
                        TargetMode::Usb
                    };
                    if novo != t.config.mode {
                        t.config.mode = novo;
                        t.salvar_config();
                        if t.transmitindo() {
                            t.status = "Conexão alterada — reinicie o streaming".into();
                        }
                    }
                }),
                options: modos_conexao
                    .iter()
                    .map(|m| RadioItem {
                        label: m.label().into(),
                        ..Default::default()
                    })
                    .collect(),
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: format!("Definir IP Wi-Fi… {}", se_vazio(&self.config.wifi_ip)),
                activate: Box::new(|t: &mut Self| t.acao_definir_ip(TargetMode::Wifi)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: format!("Definir IP USB… {}", se_vazio(&self.config.usb_ip)),
                activate: Box::new(|t: &mut Self| t.acao_definir_ip(TargetMode::Usb)),
                ..Default::default()
            }
            .into(),
        ];
        itens.push(
            SubMenu {
                label: "Conexão".into(),
                submenu: submenu_conexao,
                ..Default::default()
            }
            .into(),
        );

        itens.push(MenuItem::Separator);
        itens.push(
            StandardItem {
                label: "Sobre".into(),
                icon_name: "help-about".into(),
                activate: Box::new(|t: &mut Self| t.acao_sobre()),
                ..Default::default()
            }
            .into(),
        );
        itens.push(
            StandardItem {
                label: "Sair".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    if let Some(s) = t.session.take() {
                        s.stop();
                    }
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        );

        itens
    }
}
