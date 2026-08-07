# Plano: Monitor Virtual Linux → Android (substituto do spacedesk)

## Contexto

O usuário quer usar um tablet/celular Android como monitor secundário estendido de sua máquina Linux (Zorin OS/GNOME, sessão X11), no estilo do app comercial "spacedesk", mas com stack própria. O uso real é como **monitor de trabalho** (arrastar janelas, digitar, ver conteúdo em tempo real) — não para assistir vídeo estático — então **latência baixa é a prioridade número um**, acima de qualidade de imagem máxima. A conexão principal será Wi-Fi, com fallback por cabo USB. Não há necessidade de enviar touch/input do Android de volta ao Linux — é um projeto de exibição unidirecional (Linux → Android), o que simplifica bastante o escopo.

É um projeto **greenfield**: não existe repositório nem código ainda.

## Confirmações específicas da máquina do usuário (via `lspci`, `xrandr -q`, `groups`, `/dev/dri`)

- **GPU: AMD Lucienne (APU integrada)** → caminho de encode por hardware deve ser **VAAPI**, não NVENC.
- **Não há output X11 livre/desconectado hoje**: `xrandr -q` mostra só `eDP` (painel do notebook) e `HDMI-A-0`, ambos já conectados e em uso (setup atual já é dual monitor físico). Não existe `VIRTUAL1` nem conector "disconnected" disponível. Isso significa que a abordagem mais simples de monitor virtual (usar um output já exposto pelo driver) **não está disponível** neste hardware — será necessário ou (a) o driver `xf86-video-dummy`, ou (b) explorar `xrandr --setmonitor` para criar uma região de monitor virtual por software sobre o screen existente. Isso deve ser investigado logo no início da Fase 1, pois é a maior incerteza técnica do projeto.
- **Acesso a `/dev/dri`**: o usuário não está nos grupos estáticos `video`/`render` (`groups` não lista nenhum dos dois), mas os dispositivos (`card1`, `renderD128`) têm ACLs extras (`crw-rw----+`), tipicamente concedidas pelo `systemd-logind` ao usuário da sessão ativa. Acesso ao VAAPI provavelmente funciona sem configuração extra, mas deve ser validado cedo (rodar `vainfo` na Fase 2) — se falhar, adicionar o usuário ao grupo `render` resolve.

## Arquitetura

Duas pontas, fluxo de vídeo unidirecional Linux → Android, sem input reverso:

```
LINUX (daemon "servidor")
 [Monitor Virtual X11] → [Captura da região: XShm/XDamage] → [Encoder H.264: VAAPI, fallback x264 zerolatency]
        → [rtph264pay → udpsink]  (canal de mídia UDP)
        + [canal de controle TCP: handshake, resolução, pedido de keyframe, keepalive]
        + [anúncio mDNS/Avahi para descoberta]
              │  Wi-Fi 5GHz (ou USB tethering, mesmo transporte)
              ▼
ANDROID (cliente "monitor")
 [UdpReceiver + jitter buffer] → [MediaCodec decode HW] → [Surface/SurfaceView full-screen]
 [NsdManager p/ descoberta mDNS] + [leitor de QR como fallback de pareamento]
```

## Decisões de arquitetura recomendadas

- **Monitor virtual X11**: tentar primeiro `xrandr --setmonitor` (região virtual por software, sem precisar reiniciar o X); se insuficiente para o mutter reconhecer como monitor "de verdade" para arrastar janelas, cair para `xf86-video-dummy` via `/etc/X11/xorg.conf.d/` (precisa sudo + relogin uma vez). Isso é o item de maior risco técnico — validar antes de seguir para o resto da Fase 1.
- **Captura de tela**: XShm (MIT-SHM) na região do monitor virtual; adicionar XDamage depois para só recapturar/reencodar áreas alteradas (grande economia em desktop majoritariamente estático).
- **Codec**: H.264 via **VAAPI** (hardware AMD do usuário) como caminho principal, com `libx264 tune=zerolatency preset=ultrafast` como fallback puro-software. H.264 (não H.265/AV1) porque o decode via `MediaCodec` no Android é universal e maduro. Configurações de baixa latência: sem B-frames, intra-refresh em vez de IDR periódico pesado, GOP longo com keyframe sob demanda.
- **Transporte**: RTP/H.264 (RFC 6184) sobre **UDP** — não TCP no caminho de vídeo (retransmissão/head-of-line blocking do TCP prejudica latência sob perda de pacote em Wi-Fi), não WebRTC (NAT traversal e criptografia forte são desnecessários numa LAN doméstica, e a lib é pesada). Canal de controle separado em TCP.
- **Descoberta/pareamento**: mDNS/Zeroconf (Avahi no Linux ↔ `NsdManager` no Android) como caminho principal; QR code (IP:porta + token) como fallback para redes com isolamento de cliente Wi-Fi (AP/client isolation), que é um risco real e deve ser testado cedo na rede do usuário.
- **Fallback USB**: USB tethering do Android (não ADB reverse) — cria uma interface de rede (`usb0`) no Linux, e o mesmo stack RTP/UDP funciona sem nenhuma mudança de protocolo, só trocando a interface de rede usada.
- **Stack tecnológico**:
  - Daemon Linux em **Rust** + **GStreamer** (`gstreamer-rs`) para o pipeline de captura→encode→RTP→UDP, `x11rb`/`xcb` para XRandR/XShm/XDamage, `tokio` para o canal de controle async, `mdns-sd` para anúncio mDNS. Rust escolhido sobre C++ (mesma performance, mais segurança) e sobre Go (GC introduz pausas incompatíveis com meta de baixa latência).
  - App Android em **Kotlin** com `MediaCodec` (modo assíncrono, decode HW) renderizando direto numa `Surface`/`SurfaceView`; sockets UDP nativos com jitter buffer próprio (evitar ExoPlayer, que é orientado a VOD e adiciona buffering/latência); `NsdManager` para mDNS; leitor de QR (ZXing/ML Kit) para pareamento.

## Roadmap faseado (cada fase roda e testa ponta a ponta)

1. **Fase 0 — POC do pipeline de vídeo**: captura da tela principal inteira via XShm → `x264enc zerolatency` (software, sem depender de VAAPI ainda) → RTP/UDP via GStreamer → app Android mínimo que decodifica via MediaCodec e mostra numa SurfaceView. Objetivo: validar que dá pra ver os pixels do Linux no Android com latência aceitável (medir glass-to-glass com cronômetro + câmera).
2. **Fase 1 — Monitor virtual X11 real**: resolver a incerteza `xrandr --setmonitor` vs `xf86-video-dummy` (ver seção de confirmações acima) e capturar só a região do monitor virtual. Resolução do virtual = resolução do dispositivo Android (negociada no handshake).
3. **Fase 2 — Otimização de latência**: trocar para encode VAAPI (validar `vainfo` funciona sem sudo extra), XDamage para captura incremental, intra-refresh, ajuste de jitter buffer, feedback simples de perda/bitrate pelo canal de controle.
4. **Fase 3 — Descoberta e pareamento**: mDNS (Avahi ↔ NsdManager) com lista de daemons no app; token de pareamento; QR code como fallback (importante testar se a rede Wi-Fi do usuário tem client isolation).
5. **Fase 4 — Fallback USB**: suporte à interface de USB tethering, reusando o mesmo transporte; UI que orienta a ativar o tethering.
6. **Fase 5 — Polimento**: reconexão automática, configuração de resolução/bitrate na UI, controle de congestionamento adaptativo, cleanup do monitor virtual ao desconectar, empacotamento (`systemd --user` service + APK).

## Riscos/decisões a validar durante a implementação (não bloqueiam o início)

- Se `xrandr --setmonitor` não for suficiente para o mutter tratar como monitor real → usar `xf86-video-dummy` (requer sudo + relogin uma vez).
- Validar `vainfo` para confirmar que o acesso VAAPI funciona sem precisar adicionar o usuário ao grupo `render` manualmente.
- Testar se a rede Wi-Fi de uso tem AP/client isolation (bloquearia mDNS e talvez o próprio UDP direto) — se sim, USB tethering é o caminho garantido.
- Confirmar banda 5 GHz disponível (2.4 GHz não sustenta 1440p+ com baixa latência).
- Resolução/refresh rate do dispositivo Android alvo — define a resolução do monitor virtual e o framerate do encode.
- Escopo Wayland fica **fora da v1** (a máquina atual roda X11); manter a camada de captura atrás de uma interface para facilitar migração futura para PipeWire/`xdg-desktop-portal` se o usuário migrar para Wayland.

## Verificação de cada fase

- Fase 0: latência glass-to-glass medida com câmera + cronômetro apontando para as duas telas simultaneamente; validar que não há travamentos visuais por >1s.
- Fase 1: usuário consegue arrastar uma janela do GNOME para o monitor virtual e ela aparece corretamente enquadrada no Android.
- Fase 2: medir uso de CPU do daemon (deve cair bastante com VAAPI vs x264 software) e latência antes/depois do intra-refresh.
- Fase 3: parear um novo dispositivo Android do zero, sem digitar IP manualmente, em menos de 10s.
- Fase 4: desconectar Wi-Fi e conectar só via USB, confirmando que o vídeo continua fluindo sem reconfiguração manual de protocolo.
