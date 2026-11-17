<p align="center"><img src="docs/assets/logo.png" alt="FLM logo" width="128"></p>

# FLM — Free Linux Monitor on Android

🇬🇧 English | [🇧🇷 Português](./LEIAME.md)

Turns an Android device into a secondary display extending a Linux machine (X11), over Wi-Fi or USB cable. An open-source alternative to spacedesk, focused on low latency for use as a real working monitor.

No reverse input: the Android device is a display only, it does not send touch/mouse events back to Linux.

```
LINUX                                                        ANDROID
[vkms virtual monitor] → [ximagesrc] → [H.264 encoder] → RTP/UDP:5000 → [MediaCodec] → [SurfaceView]
[system tray]          ─────────────── resolution ─── TCP:5001 ──────→ [ControlServer]
```

## Status

Phases 0, 1 and 2 of the roadmap are complete and validated end to end (see `docs/plano-arquitetura.md`, in Portuguese). The daemon has a tray UI to start/stop streaming, turn the virtual monitor on/off, pick it, change resolution (remembered across reboots) and switch between Wi-Fi and USB targets, and can start with the session as a systemd user service. Encoding uses hardware VAAPI when the GPU supports it, with an automatic fallback to software `x264`.

---

## 1. System requirements

Tested on Zorin OS 17 / Ubuntu 24.04, GNOME Shell, AMD GPU, **X11** session.

> **Wayland is not supported.** Capture uses `ximagesrc` (X11). Check your session with `echo $XDG_SESSION_TYPE` — it must answer `x11`.

### Packages to build the daemon

```bash
sudo apt install build-essential pkg-config \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libx11-dev libxext-dev libxdamage-dev libxrandr-dev libxfixes-dev
```

### Packages to run the daemon

```bash
sudo apt install \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-ugly gstreamer1.0-vaapi \
  x11-xserver-utils zenity
```

- `gstreamer1.0-plugins-good` → `ximagesrc`, `rtph264pay`, `udpsink`
- `gstreamer1.0-plugins-ugly` → `x264enc` (software H.264 encoder, fallback)
- `gstreamer1.0-vaapi` → `vah264enc` (hardware H.264 encoder, primary path when the GPU supports it)
- `x11-xserver-utils` → `xrandr`, used to change the virtual monitor's mode and attach the provider
- `zenity` → tray dialog boxes (typing the IP)

### Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Android

- JDK 17 or newer
- Android SDK (platform 36, matching the project's `compileSdk`/`targetSdk`)

Gradle needs to know where the SDK is. Pick one of the two:

```bash
# option A: environment variable
export ANDROID_HOME=$HOME/Android/Sdk

# option B: local file (not version-controlled)
echo "sdk.dir=$HOME/Android/Sdk" > android-client/local.properties
```

---

## 2. Create the virtual monitor (once per boot)

The virtual monitor uses the **`vkms`** (Virtual Kernel Mode Setting) kernel module. It's in-tree and signed, which matters on machines with Secure Boot enabled (that rules out `evdi-dkms`). Xorg's `modesetting` driver registers vkms as a RandR *provider*, and with "Automatically adding GPU devices" enabled (the default on most distros) the module is hot-detected — **no X restart, no relogin needed**.

Two steps, required once per boot:

```bash
# 1) load the module (needs sudo — cannot be automated without an interactive password)
sudo modprobe vkms

# 2) attach vkms as an output of the main GPU (does NOT need sudo)
xrandr --listproviders
# Providers: number : 2
# Provider 0: id: 0x54  cap: 0x9, Source Output, Sink Offload ... name:AMD Radeon Graphics
# Provider 1: id: 0x425 cap: 0x2, Sink Output ...              name:modesetting
xrandr --setprovideroutputsource 0x425 0x54     # <modesetting id> <GPU id>
```

Step 2 can also be done from the tray: **Virtual monitor → Attach vkms provider**, which discovers both ids on its own. Since v0.2 the tray also does it **automatically on startup**, and turns the virtual monitor back on at the last resolution you picked (see section 4.1).

Check the result:

```bash
xrandr --listmonitors
#  2: +Virtual-1-1 1024/271x768/203+3000+1152  Virtual-1-1
```

> The output name **varies** depending on the order the provider was attached in (`Virtual-1-1`, `Virtual-1-2`, …). That's why the daemon never assumes a fixed name: the tray lists the monitors at runtime and you pick one.

After this the monitor shows up normally under GNOME **Settings → Displays**, and you can position it relative to the physical screens and drag windows onto it.

> To avoid re-running `sudo modprobe vkms` on every boot, create `/etc/modules-load.d/vkms.conf` with the line `vkms` (this is what the prebuilt `.deb` package, section 3.3, already does for you).

---

## 3. Install / build

### 3.1 Daemon (building from source)

```bash
cd daemon
cargo build --release
# binary at daemon/target/release/flm-daemon
```

### 3.2 Android app

```bash
cd android-client
./gradlew assembleDebug
# APK at android-client/app/build/outputs/apk/debug/app-debug.apk
```

Install it on the device (over USB, with USB debugging enabled):

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

If `adb` doesn't see the device due to missing udev permissions, you need a `udev` rule with your device manufacturer's vendor id (e.g. `22b8` for Motorola) — a machine setup detail, not a project one.

### 3.3 Prebuilt `.deb` package (faster alternative)

If you have a prebuilt `.deb` (`flm-daemon_<version>_amd64.deb`), installing it is much faster than building — no Rust or `-dev` libraries needed:

```bash
sudo apt install ./flm-daemon_<version>_amd64.deb
```

The package already ships `/usr/lib/modules-load.d/flm-vkms.conf` (loads `vkms` on every boot automatically) and runs `modprobe vkms` once during post-install. It also ships the user service `/usr/lib/systemd/user/flm-daemon.service` and enables it for all users, so the tray starts by itself from the next login on (section 4.4).

To build the package yourself (no sudo needed):

```bash
cd daemon
dist/build-deb.sh
# package at daemon/target/flm-daemon_<version>_amd64.deb
```

---

## 4. Usage

### 4.1 System tray

```bash
flm-daemon
# or, if built locally:
./daemon/target/release/flm-daemon
```

With no arguments, the daemon registers a tray icon (a monitor) and runs in the background. The menu has:

| Item | What it does |
|---|---|
| **(status)** | Three info lines: running/stopped and which IP, monitor and resolution are in use, active connection |
| **Start / Stop streaming** | Turns the pipeline on and off |
| **Enable / Disable virtual monitor** | Turns the virtual monitor on (to the right of the primary screen, at the saved resolution) or off (windows go back to the real screens; streaming stops). The choice is remembered |
| **Virtual monitor** | Lists detected RandR monitors; pick the virtual one. Also has "Attach vkms provider" |
| **Resolution** | Available modes for the virtual monitor; applying changes the mode, re-notifies the Android app and saves it for the next start |
| **Connection** | Switches between **Wi-Fi** and **USB (tethering)** targets and lets you edit each IP |
| **About** | Summary of current config and ports in use |
| **Quit** | Stops streaming and exits |

Typical flow:

1. Load vkms and attach the provider (section 2).
2. Open the FLM app on Android and note the device's IP.
3. In the tray: **Connection → Set Wi-Fi IP…** and paste the IP.
4. **Virtual monitor →** pick `Virtual-1-1` (usually pre-selected already).
5. **Resolution →** pick the resolution you want.
6. **Start streaming.**

Configuration lives at `~/.config/flm/config.toml` and is remembered across runs:

```toml
monitor = "Virtual-1-1"
mode = "wifi"          # or "usb"
wifi_ip = "192.168.0.42"
usb_ip = "192.168.210.74"
framerate = 30
resolucao = "1920x1080"  # last resolution picked in the tray
virtual_ativo = true     # virtual monitor on/off, restored on startup
```

On startup the tray restores the virtual monitor as you left it: it re-attaches the vkms provider (that link is lost on every boot), then turns the monitor on at the saved resolution — or keeps it off.

### 4.2 Wi-Fi vs USB

There's no automatic discovery (mDNS) yet — that's Phase 3 of the roadmap. In practice the two modes are just **two saved IPs**, since the transport is identical: USB tethering shows up on Linux as just another network interface, and the same RTP/UDP flows over it without any protocol change.

To use the cable: enable **USB tethering** on Android, find the IP with `ip addr show usb0` (the gateway is usually the `.1` of the range; the device's IP is what the app shows), fill it in under **Connection → Set USB IP…** and select **USB (tethering)**.

### 4.3 One-shot mode (command line)

The original behavior is still available, useful for debugging the pipeline without involving D-Bus, the tray or the GNOME extension:

```bash
flm-daemon <android-ip> [monitor-name]   # streams until Ctrl+C
flm-daemon --list-monitors               # lists RandR monitors
flm-daemon --help
```

> One-shot mode does **not** use the control channel: the Android app falls back to 1024x768 after ~4s. To test other resolutions, use the tray.

### 4.4 Start with the session (always on)

`daemon/dist/flm-daemon.service` is a systemd **user** service that starts the tray with the graphical session and restarts it if it crashes (**Quit** in the menu exits cleanly and is *not* restarted until the next login). The `.deb` installs and enables it for you. When building from source:

```bash
mkdir -p ~/.config/systemd/user
cp daemon/dist/flm-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now flm-daemon.service
```

The service runs `/usr/bin/flm-daemon`; adjust `ExecStart` if your binary lives elsewhere. Logs: `journalctl --user -u flm-daemon`.

---

## 5. Tray icon on GNOME Shell — heads up

GNOME Shell has **not supported tray icons natively** since version 3.26. You need an extension implementing the *StatusNotifierItem/AppIndicator* spec.

Some distros (Zorin OS, for instance) already ship an indicator extension enabled by default. On **vanilla GNOME** (Ubuntu, Fedora…), install and enable one:

```bash
sudo apt install gnome-shell-extension-appindicator
# then: log out and back in (on X11, Alt+F2 → "r" → Enter also works)
gnome-extensions enable ubuntu-appindicators@ubuntu.com
# or, depending on the distro:
gnome-extensions enable appindicatorsupport@rgcjonas.gmail.com
```

Alternative: install "AppIndicator and KStatusNotifierItem Support" from <https://extensions.gnome.org>.

To check everything is fine:

```bash
gnome-extensions list | grep -i appindicator
gnome-extensions info <extension-name>   # should say Enabled: Yes / State: ACTIVE

# the definitive check: someone needs to be serving the watcher on the bus
busctl --user list | grep StatusNotifierWatcher
# org.kde.StatusNotifierWatcher   ...   gnome-shell   ...
```

If no watcher is present, `flm-daemon` **fails to start with an explicit message** rather than coming up invisibly. In that case, use one-shot mode (section 4.3) while you sort out the extension.

---

## 6. How resolution gets to Android

Hardware decoders (Qualcomm Venus, for example) often **don't renegotiate dimensions via SPS**: if `MediaCodec` is created with a size different from what arrives in the stream, it simply produces no image. Since the tray lets you change the virtual monitor's mode, the resolution needs to reach the app through another path.

A simple TCP control channel was implemented:

1. Before bringing up the pipeline, the daemon connects to `<android-ip>:5001`.
2. It sends an ASCII line: `FLM/1 <width>x<height>\n`.
3. It closes the connection and only then starts GStreamer.
4. The app, which was listening on that port, recreates `MediaCodec` with the received dimensions.

When changing resolution while streaming, the tray does the full cycle: stop the pipeline → apply the mode with `xrandr` → resend the resolution → bring the pipeline back up. The image returns in about 1 second (the encoder sends SPS/PPS every second and one keyframe per GOP).

**Compatibility:** if the control channel fails (app closed, older APK, firewall), streaming **starts anyway** and the tray shows a warning — it's not a fatal error. On the app side, if no control message arrives within ~4s, it assumes 1024x768 and proceeds.

### Ports

| Port | Protocol | Direction | Use |
|---|---|---|---|
| 5000 | UDP | Linux → Android | RTP/H.264 video |
| 5001 | TCP | Linux → Android | Control channel (resolution) |

Both need to be open along the path. On a normal home LAN there's nothing to configure; networks with *client isolation* block both (in that case, use USB).

---

## 7. Troubleshooting

**The tray icon doesn't show up.** See section 5. Confirm with `busctl --user list | grep StatusNotifierWatcher`.

**"monitor X does not exist" / empty monitor list.** vkms isn't loaded or wasn't attached. Run `sudo modprobe vkms` and then **Virtual monitor → Attach vkms provider**.

**Black screen on Android, no error on the daemon.** Almost always a resolution mismatch. Make sure the app is open *before* hitting Start (the control channel needs a listener), and check the log: `adb logcat -s FlmClient`. It should show `decoder configurado em WxH`.

**Corrupted image, magenta blocks.** Already diagnosed and fixed on the x264 encoder: multiple slices per frame with `tune=zerolatency` on a multi-core CPU made the Qualcomm decoder decode only the first slice and do error concealment on the rest. The fix (`threads=1`) is in the pipeline. The same applies to color format: the pipeline converts to `I420` (4:2:0) because `ximagesrc` delivers `Y444`, which phone decoders don't support.

**High latency / high CPU.** Check whether the active encoder is VAAPI and not the x264 fallback (the tray's **About** menu shows which one is active). If it fell back, run `vainfo` to check VAAPI access is working (sometimes you need to add the user to the `render` group).

**Nothing reaches Android.** Test the network path: `nc -zv <ip> 5001` for the control channel. If the Wi-Fi network has client isolation, use USB tethering.

---

## 8. Known limitations

- **X11 only.** Wayland is out of scope for v1.
- **No automatic discovery.** The IP is typed in by hand (mDNS is Phase 3 of the roadmap).
- **One-way, ephemeral control channel.** It only carries resolution, from Linux to Android. No keepalive, keyframe requests, automatic reconnection or loss feedback — planned for future phases.
- **Changing resolution restarts the pipeline**, with ~1s of frozen screen.
- **No link-loss detection.** Video is one-way UDP: if the app is closed or the network drops mid-stream, the daemon keeps sending and doesn't notice.
- **`sudo modprobe vkms` on every boot**, unless you create `/etc/modules-load.d/vkms.conf` (section 2) or use the `.deb` package (section 3.3), which already does this.
- **No reverse input** — a scope decision, not a limitation to be fixed.

---

## Contributing

Open to contributions. There's no formal contribution guide yet (`CONTRIBUTING.md`) — open an issue or PR.

## License

GPLv3 — see [LICENSE](./LICENSE). Copyright (C) 2026 Bruno S Sampaio.
