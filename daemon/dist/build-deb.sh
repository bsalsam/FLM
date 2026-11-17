#!/bin/bash
# Gera o pacote .deb do flm-daemon (não precisa de sudo).
#
#   cd daemon && dist/build-deb.sh            # compila em release e empacota
#   dist/build-deb.sh --sem-compilar          # usa o binário já compilado
#
# Saída: daemon/target/flm-daemon_<versão>_amd64.deb
set -euo pipefail

DIST="$(cd "$(dirname "$0")" && pwd)"
DAEMON="$(dirname "$DIST")"
REPO="$(dirname "$DAEMON")"
VERSAO="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$DAEMON/Cargo.toml" | head -1)"
ARQ=amd64

if [ "${1:-}" != "--sem-compilar" ]; then
    (cd "$DAEMON" && cargo build --release)
fi
BIN="$DAEMON/target/release/flm-daemon"
[ -x "$BIN" ] || { echo "binário não encontrado: $BIN" >&2; exit 1; }

RAIZ="$(mktemp -d)"
trap 'rm -rf "$RAIZ"' EXIT

install -Dm755 "$BIN" "$RAIZ/usr/bin/flm-daemon"
install -Dm644 "$DIST/deb/flm-vkms.conf" "$RAIZ/usr/lib/modules-load.d/flm-vkms.conf"
install -Dm644 "$DIST/flm-daemon.service" "$RAIZ/usr/lib/systemd/user/flm-daemon.service"
install -Dm644 "$DIST/deb/flm.desktop" "$RAIZ/usr/share/applications/flm.desktop"
install -Dm644 "$REPO/README.md" "$RAIZ/usr/share/doc/flm-daemon/README.md"
install -Dm644 "$REPO/LEIAME.md" "$RAIZ/usr/share/doc/flm-daemon/LEIAME.md"
for s in 48 64 128 256 512; do
    install -Dm644 "$DIST/deb/icons/flm-$s.png" "$RAIZ/usr/share/icons/hicolor/${s}x${s}/apps/flm.png"
done

mkdir -p "$RAIZ/DEBIAN"
cat > "$RAIZ/DEBIAN/control" <<EOF
Package: flm-daemon
Version: $VERSAO
Section: utils
Priority: optional
Architecture: $ARQ
Maintainer: Bruno S Sampaio <brunosalwork@gmail.com>
Depends: libgstreamer1.0-0, libglib2.0-0, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-plugins-ugly, x11-xserver-utils, zenity
Recommends: mesa-va-drivers, libnotify-bin
Description: FLM — Free Linux Monitor on Android (daemon)
 Transforma um dispositivo Android num monitor secundário estendido da
 máquina Linux (X11), via Wi-Fi ou cabo USB (tethering). Alternativa
 open source (GPLv3) ao spacedesk.
 .
 Inclui o ícone de bandeja (requer a extensão AppIndicator no GNOME),
 o serviço de usuário que o inicia junto com a sessão gráfica, o modo
 one-shot de linha de comando e o carregamento automático do módulo de
 kernel vkms. O app Android (APK) é distribuído separadamente.
EOF

cat > "$RAIZ/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
# Carrega o vkms já nesta sessão (o modules-load.d cuida dos próximos boots).
modprobe vkms 2>/dev/null || true
# Liga o serviço de usuário para todos os usuários (vale a partir do próximo
# login; quem quiser já agora: systemctl --user start flm-daemon).
if [ "$1" = "configure" ] && command -v systemctl >/dev/null; then
    systemctl --global enable flm-daemon.service >/dev/null 2>&1 || true
fi
exit 0
EOF

cat > "$RAIZ/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "remove" ] && command -v systemctl >/dev/null; then
    systemctl --global disable flm-daemon.service >/dev/null 2>&1 || true
fi
exit 0
EOF
chmod 755 "$RAIZ/DEBIAN/postinst" "$RAIZ/DEBIAN/prerm"

SAIDA="$DAEMON/target/flm-daemon_${VERSAO}_${ARQ}.deb"
dpkg-deb --root-owner-group --build "$RAIZ" "$SAIDA" >/dev/null
echo "pacote gerado: $SAIDA"
