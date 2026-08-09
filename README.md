# FLM — Free Linux Monitor on Android

Transforma um dispositivo Android em um monitor secundário estendido de uma máquina Linux (X11), via Wi-Fi ou cabo USB. Alternativa open source ao spacedesk, focada em baixa latência para uso como monitor de trabalho de verdade.

Sem input reverso: o Android é um display, não envia toque/mouse de volta ao Linux.

```
LINUX                                                        ANDROID
[monitor virtual vkms] → [ximagesrc] → [x264enc] → RTP/UDP:5000 → [MediaCodec] → [SurfaceView]
[bandeja do sistema]   ────────────── resolução ── TCP:5001 ────→ [ControlServer]
```

## Status

Fases 0 e 1 do roadmap concluídas e validadas ponta a ponta (ver `docs/plano-arquitetura.md`). O daemon tem uma interface de bandeja para iniciar/parar o streaming, escolher o monitor virtual, trocar a resolução e alternar entre os alvos Wi-Fi e USB.

---

## 1. Pré-requisitos do sistema

Testado em Zorin OS 17 (base Ubuntu 24.04), GNOME Shell 46 em sessão **X11**, GPU AMD Lucienne.

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
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-ugly \
  x11-xserver-utils zenity
```

- `gstreamer1.0-plugins-good` → `ximagesrc`, `rtph264pay`, `udpsink`
- `gstreamer1.0-plugins-ugly` → `x264enc` (encoder H.264 por software)
- `x11-xserver-utils` → `xrandr`, usado para trocar o modo do monitor virtual e anexar o provider
- `zenity` → caixas de diálogo da bandeja (digitar o IP)

### Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Android

- JDK 17 ou superior (esta máquina usa o OpenJDK 21 do sistema)
- Android SDK (plataforma 36). Se você já tem o Android Studio, o SDK costuma ficar em `~/Android/Sdk`.

O Gradle precisa saber onde está o SDK. Escolha uma das duas formas:

```bash
# opção A: variável de ambiente
export ANDROID_HOME=$HOME/Android/Sdk

# opção B: arquivo local (não versionado)
echo "sdk.dir=$HOME/Android/Sdk" > android-client/local.properties
```

---

## 2. Criar o monitor virtual (a cada boot)

O monitor virtual usa o módulo de kernel **`vkms`** (Virtual Kernel Mode Setting). Ele é in-tree e assinado, o que importa nesta máquina porque o Secure Boot está ligado (inviabiliza `evdi-dkms`). O driver `modesetting` do Xorg registra o vkms como um *provider* RandR, e o Xorg está com "Automatically adding GPU devices" ativo — então o módulo é detectado a quente, **sem reiniciar o X e sem relogin**.

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

---

## 3. Compilar

### Daemon

```bash
cd daemon
cargo build --release
# binário em daemon/target/release/flm-daemon
```

### App Android

```bash
cd android-client
./gradlew assembleDebug
# APK em android-client/app/build/outputs/apk/debug/app-debug.apk
```

Instalar no aparelho (via USB, com depuração USB ligada):

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Se o `adb` não enxergar o aparelho por falta de permissão de udev, existe um script de referência fora do repositório em `~/fix-adb-udev.sh` (regra para o vendor id `22b8`, Motorola). É um ajuste de máquina, não do projeto.

---

## 4. Usar

### 4.1 Bandeja do sistema

```bash
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

**Nesta máquina (Zorin OS) já está tudo pronto:** o Zorin embarca sua própria versão, `zorin-appindicator@zorinos.com` (pacote `gnome-shell-extension-zorin-appindicator`), habilitada por padrão. Nada a fazer.

Para conferir:

```bash
gnome-extensions list | grep -i appindicator
gnome-extensions info zorin-appindicator@zorinos.com   # deve dizer Habilitado: Sim / Estado: ACTIVE

# checagem definitiva: alguém precisa estar servindo o watcher no barramento
busctl --user list | grep StatusNotifierWatcher
# org.kde.StatusNotifierWatcher   ...   gnome-shell   ...
```

**Em GNOME baunilha (Ubuntu, Fedora…)**, instale e ative:

```bash
sudo apt install gnome-shell-extension-appindicator
# depois: logout e login (em X11, Alt+F2 → "r" → Enter também funciona)
gnome-extensions enable ubuntu-appindicators@ubuntu.com
# ou, conforme a distro:
gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com
```

Alternativa: instalar "AppIndicator and KStatusNotifierItem Support" por <https://extensions.gnome.org>.

Se nenhum watcher estiver presente, o `flm-daemon` **falha ao iniciar com uma mensagem explícita** em vez de subir invisível. Nesse caso, use o modo one-shot (seção 4.3) enquanto resolve a extensão.

---

## 6. Como a resolução atravessa para o Android

O decoder de hardware Qualcomm (Venus, no Motorola G8 Power) **não renegocia dimensões pelo SPS**: se o `MediaCodec` for criado com um tamanho diferente do que chega no stream, ele simplesmente não produz imagem. Como a bandeja permite trocar o modo do monitor virtual, a resolução precisa chegar ao app.

Foi implementada a versão mínima do canal de controle TCP previsto na arquitetura:

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

**Imagem corrompida, blocos magenta.** Já foi diagnosticado e corrigido: era o `x264enc` gerando múltiplas slices por frame com `tune=zerolatency` em CPU multi-core — o decoder Qualcomm só decodificava a primeira e fazia *error concealment* no resto. A correção (`threads=1`) está no pipeline. Se reaparecer ao mexer no encoder, é o primeiro suspeito. O mesmo vale para o formato: o pipeline força `I420` (4:2:0) porque o `ximagesrc` entrega `Y444`, que decoders de celular não suportam.

**Latência alta / CPU alto.** Esperado: o encode ainda é por software (`x264enc`). A migração para VAAPI é a Fase 2 do roadmap.

**Nada chega no Android.** Teste o caminho de rede: `nc -zv <ip> 5001` para o controle. Se a rede Wi-Fi tiver isolamento de clientes, use USB tethering.

---

## 8. Limitações conhecidas

- **Só X11.** Wayland está fora do escopo da v1.
- **Encode por software.** VAAPI (Fase 2) ainda não foi implementado.
- **Sem descoberta automática.** O IP é digitado à mão (mDNS é a Fase 3).
- **Canal de controle unidirecional e efêmero.** Só carrega a resolução, do Linux para o Android. Não há keepalive, pedido de keyframe, reconexão automática nem feedback de perda — tudo isso está previsto nas Fases 2/3.
- **A troca de resolução reinicia o pipeline**, com ~1s de tela parada.
- **`sudo modprobe vkms` a cada boot.** Não é automatizável sem senha interativa; para eliminar o passo, dá para criar um `/etc/modules-load.d/vkms.conf` com a linha `vkms`.
- **Sem input reverso** — por decisão de escopo, não é uma limitação a corrigir.

---

## Contribuindo

Projeto aberto a contribuições. Ainda não há guia formal de contribuição (`CONTRIBUTING.md`) — abra uma issue ou PR quando o repositório for publicado.

## Licença

GPLv3 — veja [LICENSE](./LICENSE). Copyright (C) 2026 Bruno S Sampaio.
