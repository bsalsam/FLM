# FLM — Free Linux Monitor on Android

🇧🇷 Português | [🇬🇧 English](./README.md)

Transforma um dispositivo Android em um monitor secundário estendido de uma máquina Linux (X11), via Wi-Fi ou cabo USB. Alternativa open source ao spacedesk, focada em baixa latência para uso como monitor de trabalho de verdade.

Sem input reverso: o Android é um display, não envia toque/mouse de volta ao Linux.

```
LINUX                                                        ANDROID
[monitor virtual vkms] → [ximagesrc] → [encoder H.264] → RTP/UDP:5000 → [MediaCodec] → [SurfaceView]
[bandeja do sistema]   ────────────── resolução ──────  TCP:5001 ────→ [ControlServer]
```

## Status

Fases 0, 1 e 2 do roadmap concluídas e validadas ponta a ponta (ver `docs/plano-arquitetura.md`). O daemon tem uma interface de bandeja para iniciar/parar o streaming, escolher o monitor virtual, trocar a resolução e alternar entre os alvos Wi-Fi e USB. O encode usa hardware VAAPI quando a GPU oferece suporte, com fallback automático para `x264` por software.

---

## 1. Pré-requisitos do sistema

Testado em Zorin OS 17 / Ubuntu 24.04, GNOME Shell, GPU AMD, sessão **X11**.

> **Wayland não é suportado.** A captura usa `ximagesrc` (X11). Confira a sessão com `echo $XDG_SESSION_TYPE` — precisa responder `x11`.

### Pacotes para compilar o daemon

```bash
sudo apt install build-essential pkg-config \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libx11-dev libxext-dev libxdamage-dev libxrandr-dev libxfixes-dev
```

### Pacotes para rodar o daemon

```bash
sudo apt install \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-ugly gstreamer1.0-vaapi \
  x11-xserver-utils zenity
```

- `gstreamer1.0-plugins-good` → `ximagesrc`, `rtph264pay`, `udpsink`
- `gstreamer1.0-plugins-ugly` → `x264enc` (encoder H.264 por software, fallback)
- `gstreamer1.0-vaapi` → `vah264enc` (encoder H.264 por hardware, caminho principal quando a GPU suporta)
- `x11-xserver-utils` → `xrandr`, usado para trocar o modo do monitor virtual e anexar o provider
- `zenity` → caixas de diálogo da bandeja (digitar o IP)

### Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Android

- JDK 17 ou superior
- Android SDK (plataforma 36, `compileSdk`/`targetSdk` do projeto)

O Gradle precisa saber onde está o SDK. Escolha uma das duas formas:

```bash
# opção A: variável de ambiente
export ANDROID_HOME=$HOME/Android/Sdk

# opção B: arquivo local (não versionado)
echo "sdk.dir=$HOME/Android/Sdk" > android-client/local.properties
```

---

## 2. Criar o monitor virtual (a cada boot)

O monitor virtual usa o módulo de kernel **`vkms`** (Virtual Kernel Mode Setting). Ele é in-tree e assinado, o que importa em máquinas com Secure Boot ligado (inviabiliza `evdi-dkms`). O driver `modesetting` do Xorg registra o vkms como um *provider* RandR, e com "Automatically adding GPU devices" ativo (padrão na maioria das distros) o módulo é detectado a quente, **sem reiniciar o X e sem relogin**.

São dois passos, necessários uma vez por boot:

```bash
# 1) carregar o módulo (exige sudo — não dá para automatizar sem senha interativa)
sudo modprobe vkms

# 2) anexar o vkms como saída da GPU principal (NÃO exige sudo)
xrandr --listproviders
# Providers: number : 2
# Provider 0: id: 0x54  cap: 0x9, Source Output, Sink Offload ... name:AMD Radeon Graphics
# Provider 1: id: 0x425 cap: 0x2, Sink Output ...              name:modesetting
xrandr --setprovideroutputsource 0x425 0x54     # <id do modesetting> <id da GPU>
```

O passo 2 também pode ser feito pela bandeja: menu **Monitor virtual → Anexar provider vkms**, que descobre os dois ids sozinho.

Confira o resultado:

```bash
xrandr --listmonitors
#  2: +Virtual-1-1 1024/271x768/203+3000+1152  Virtual-1-1
```

> O nome do output **varia** conforme a ordem em que o provider é anexado (`Virtual-1-1`, `Virtual-1-2`, …). Por isso o daemon nunca assume um nome fixo: a bandeja lista os monitores em tempo de execução e você escolhe.

Depois disso o monitor aparece normalmente em **Configurações → Telas** do GNOME, e você pode posicioná-lo em relação às telas físicas e arrastar janelas para ele.

> Pra não repetir o `sudo modprobe vkms` a cada boot, crie `/etc/modules-load.d/vkms.conf` com a linha `vkms` (é o que o pacote `.deb` pré-compilado, seção 3.3, já faz sozinho).

---

## 3. Instalar / compilar

### 3.1 Daemon (compilando)

```bash
cd daemon
cargo build --release
# binário em daemon/target/release/flm-daemon
```

### 3.2 App Android

```bash
cd android-client
./gradlew assembleDebug
# APK em android-client/app/build/outputs/apk/debug/app-debug.apk
```

Instalar no aparelho (via USB, com depuração USB ligada):

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Se o `adb` não enxergar o aparelho por falta de permissão de udev, é preciso uma regra `udev` com o vendor id do fabricante do seu aparelho (ex.: `22b8` pra Motorola) — ajuste de máquina, não do projeto.

### 3.3 Pacote `.deb` pré-compilado (alternativa mais rápida)

Se você tem um `.deb` gerado (`flm-daemon_<versão>_amd64.deb`), instalar por ele é bem mais rápido que compilar — não precisa de Rust nem das libs `-dev`:

```bash
sudo apt install ./flm-daemon_<versão>_amd64.deb
```

O pacote já inclui `/usr/lib/modules-load.d/flm-vkms.conf` (carrega o `vkms` sozinho em todo boot) e roda `modprobe vkms` uma vez no pós-instalação.

---

## 4. Usar

### 4.1 Bandeja do sistema

```bash
flm-daemon
# ou, se compilado localmente:
./daemon/target/release/flm-daemon
```

Sem argumentos, o daemon registra um ícone de bandeja (um monitor) e fica em segundo plano. O menu tem:

| Item | O que faz |
|---|---|
| **(status)** | Três linhas informativas: rodando/parado e para qual IP, monitor e resolução em uso, conexão ativa |
| **Iniciar / Parar streaming** | Liga e desliga o pipeline |
| **Monitor virtual** | Lista os monitores RandR detectados; escolha o virtual. Também tem "Anexar provider vkms" |
| **Resolução** | Modos disponíveis do monitor virtual; aplicar troca o modo e reavisa o app Android |
| **Conexão** | Alterna entre os alvos **Wi-Fi** e **USB (tethering)** e permite editar o IP de cada um |
| **Sobre** | Resumo da configuração atual e das portas usadas |
| **Sair** | Para o streaming e encerra |

Fluxo típico:

1. Carregue o vkms e anexe o provider (seção 2).
2. Abra o app FLM no Android e veja o IP do aparelho.
3. Na bandeja: **Conexão → Definir IP Wi-Fi…** e cole o IP.
4. **Monitor virtual →** escolha `Virtual-1-1` (normalmente já vem pré-selecionado).
5. **Resolução →** escolha a resolução desejada.
6. **Iniciar streaming.**

A configuração fica em `~/.config/flm/config.toml` e é lembrada entre execuções:

```toml
monitor = "Virtual-1-1"
mode = "wifi"          # ou "usb"
wifi_ip = "192.168.0.42"
usb_ip = "192.168.210.74"
framerate = 30
```

### 4.2 Wi-Fi x USB

Não há descoberta automática (mDNS) ainda — isso é a Fase 3 do roadmap. Na prática os dois modos são só **dois IPs salvos**, porque o transporte é idêntico: o tethering USB aparece no Linux como mais uma interface de rede, e o mesmo RTP/UDP flui por ela sem nenhuma mudança de protocolo.

Para usar o cabo: ative **Tethering USB** no Android, descubra o IP com `ip addr show usb0` (o gateway costuma ser o `.1` da faixa; o IP do aparelho é o que o app mostra), preencha em **Conexão → Definir IP USB…** e selecione **USB (tethering)**.

### 4.3 Modo one-shot (linha de comando)

O comportamento antigo continua disponível, útil para depurar o pipeline sem envolver D-Bus, bandeja ou extensão do GNOME:

```bash
flm-daemon <ip-do-android> [nome-do-monitor]   # transmite até Ctrl+C
flm-daemon --list-monitors                     # lista os monitores RandR
flm-daemon --help
```

> O modo one-shot **não** usa o canal de controle: o app Android cai no fallback de 1024x768 depois de ~4s. Para testar outras resoluções, use a bandeja.

---

## 5. Bandeja no GNOME Shell — atenção

O GNOME Shell **não suporta ícones de bandeja nativamente** desde a versão 3.26. É preciso uma extensão que implemente o *StatusNotifierItem/AppIndicator*.

Algumas distros (Zorin OS, por exemplo) já vêm com uma extensão de indicador habilitada por padrão. Em **GNOME baunilha** (Ubuntu, Fedora…), instale e ative:

```bash
sudo apt install gnome-shell-extension-appindicator
# depois: logout e login (em X11, Alt+F2 → "r" → Enter também funciona)
gnome-extensions enable ubuntu-appindicators@ubuntu.com
# ou, conforme a distro:
gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com
```

Alternativa: instalar "AppIndicator and KStatusNotifierItem Support" por <https://extensions.gnome.org>.

Para conferir se está tudo certo:

```bash
gnome-extensions list | grep -i appindicator
gnome-extensions info <nome-da-extensão>   # deve dizer Habilitado: Sim / Estado: ACTIVE

# checagem definitiva: alguém precisa estar servindo o watcher no barramento
busctl --user list | grep StatusNotifierWatcher
# org.kde.StatusNotifierWatcher   ...   gnome-shell   ...
```

Se nenhum watcher estiver presente, o `flm-daemon` **falha ao iniciar com uma mensagem explícita** em vez de subir invisível. Nesse caso, use o modo one-shot (seção 4.3) enquanto resolve a extensão.

---

## 6. Como a resolução atravessa para o Android

Decoders de hardware (Qualcomm Venus, por exemplo) costumam **não renegociar dimensões pelo SPS**: se o `MediaCodec` for criado com um tamanho diferente do que chega no stream, ele simplesmente não produz imagem. Como a bandeja permite trocar o modo do monitor virtual, a resolução precisa chegar ao app por outro caminho.

Foi implementado um canal de controle TCP simples:

1. Antes de subir o pipeline, o daemon conecta em `<ip-do-android>:5001`.
2. Envia uma linha ASCII: `FLM/1 <largura>x<altura>\n`.
3. Fecha a conexão e só então inicia o GStreamer.
4. O app, que estava escutando nessa porta, recria o `MediaCodec` com as dimensões recebidas.

Ao trocar a resolução com o streaming ligado, a bandeja faz o ciclo completo: para o pipeline → aplica o modo com `xrandr` → reenvia a resolução → sobe o pipeline de novo. A imagem volta em aproximadamente 1 segundo (o encoder manda SPS/PPS a cada segundo e um keyframe por GOP).

**Compatibilidade:** se o canal de controle falhar (app fechado, APK antigo, firewall), o streaming **começa mesmo assim** e a bandeja mostra um aviso — não é um erro fatal. Do lado do app, se nenhuma mensagem de controle chegar em ~4s, ele assume 1024x768 e segue.

### Portas

| Porta | Protocolo | Sentido | Uso |
|---|---|---|---|
| 5000 | UDP | Linux → Android | Vídeo RTP/H.264 |
| 5001 | TCP | Linux → Android | Canal de controle (resolução) |

Ambas precisam estar liberadas no caminho. Numa LAN doméstica normal não há o que configurar; redes com *client isolation* bloqueiam os dois (nesse caso, use USB).

---

## 7. Solução de problemas

**O ícone não aparece na bandeja.** Veja a seção 5. Confirme com `busctl --user list | grep StatusNotifierWatcher`.

**"monitor X não existe" / lista de monitores vazia.** O vkms não está carregado ou não foi anexado. Rode `sudo modprobe vkms` e depois **Monitor virtual → Anexar provider vkms**.

**Tela preta no Android, sem erro no daemon.** Quase sempre é mismatch de resolução. Confirme que o app está aberto *antes* de dar Iniciar (o canal de controle precisa de alguém escutando), e veja o log: `adb logcat -s FlmClient`. Deve aparecer `decoder configurado em WxH`.

**Imagem corrompida, blocos magenta.** Já foi diagnosticado e corrigido no encoder x264: múltiplas *slices* por frame com `tune=zerolatency` em CPU multi-core faziam o decoder Qualcomm decodificar só a primeira slice e fazer *error concealment* no resto. A correção (`threads=1`) está no pipeline. O mesmo vale para o formato de cor: o pipeline converte pra `I420` (4:2:0) porque o `ximagesrc` entrega `Y444`, que decoders de celular não suportam.

**Latência alta / CPU alto.** Confira se o encoder em uso é o VAAPI e não o fallback x264 (menu **Sobre** na bandeja mostra qual está ativo). Se caiu pro fallback, rode `vainfo` pra checar se o acesso VAAPI está funcionando (às vezes é preciso adicionar o usuário ao grupo `render`).

**Nada chega no Android.** Teste o caminho de rede: `nc -zv <ip> 5001` para o controle. Se a rede Wi-Fi tiver isolamento de clientes, use USB tethering.

---

## 8. Limitações conhecidas

- **Só X11.** Wayland está fora do escopo da v1.
- **Sem descoberta automática.** O IP é digitado à mão (mDNS é a Fase 3 do roadmap).
- **Canal de controle unidirecional e efêmero.** Só carrega a resolução, do Linux para o Android. Não há keepalive, pedido de keyframe, reconexão automática nem feedback de perda — previsto pra fases futuras.
- **A troca de resolução reinicia o pipeline**, com ~1s de tela parada.
- **`sudo modprobe vkms` a cada boot**, a menos que você crie o `/etc/modules-load.d/vkms.conf` (seção 2) ou use o pacote `.deb` (seção 3.3), que já faz isso.
- **Sem input reverso** — por decisão de escopo, não é uma limitação a corrigir.

---

## Contribuindo

Projeto aberto a contribuições. Ainda não há guia formal de contribuição (`CONTRIBUTING.md`) — abra uma issue ou PR.

## Licença

GPLv3 — veja [LICENSE](./LICENSE). Copyright (C) 2026 Bruno S Sampaio.
