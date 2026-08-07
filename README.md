# FLM — Free Linux Monitor on Android

Transforma um dispositivo Android em um monitor secundário estendido de uma máquina Linux (X11), via Wi-Fi ou cabo USB. Alternativa open source ao spacedesk, focada em baixa latência para uso como monitor de trabalho de verdade.

Sem input reverso: o Android é um display, não envia toque/mouse de volta ao Linux.

## Status

Projeto em desenvolvimento inicial (Fase 0 do roadmap: prova de conceito do pipeline de vídeo). Veja o plano de arquitetura completo em `docs/plano-arquitetura.md`.

## Estrutura

- `daemon/` — daemon Linux (Rust): cria o monitor virtual, captura a tela, codifica em H.264 e envia via RTP/UDP.
- `android-client/` — app Android (Kotlin): recebe o stream, decodifica e exibe em tela cheia.

## Requisitos de build

Daemon (Linux):
- Rust (via [rustup](https://rustup.rs))
- `pkg-config`, `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, `libgstreamer-plugins-bad1.0-dev`, `libx11-dev`, `libxext-dev`, `libxdamage-dev`, `libxrandr-dev`, `libxfixes-dev`

```
cd daemon
cargo build
```

Android client:
- Android Studio (a definir versão mínima de SDK/API conforme desenvolvimento avança)

## Contribuindo

Projeto aberto a contribuições. Ainda não há guia formal de contribuição (`CONTRIBUTING.md`) — abra uma issue ou PR quando o repositório for publicado.

## Licença

GPLv3 — veja [LICENSE](./LICENSE). Copyright (C) 2026 Bruno S Sampaio.
