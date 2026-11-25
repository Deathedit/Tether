# Status

Milestones 1 (discover), 2 (pair), 3 (first picture), 4 (play) and 5 (desktop
UI): sound, controllers and rumble, in the desktop app.

Milestones 6 (Android core) and 7 (Android app): the Android app (APK from
CI, see [building](building.md#android)), tested on an AYN Thor with a PS5:

- Home screen from the design: the selected console, console cards and
  A/Y/X hints; the controller picks (left/right), connects (A), puts the
  console to rest (Y) and opens settings (X).
- Streams with hardware video decoding (MediaCodec drawing straight onto
  the screen), sound, the Thor's built-in controls (or any controller) and
  rumble.
- On the Thor's bottom screen: at home, the network to the selected console
  (latency graph, jitter) and big Connect / Rest mode / Settings buttons; in
  a stream, live latency, bitrate, frame rate and loss, a DualSense touchpad
  (tap to click) and Home, Mic, Capture and Disconnect. On phones that panel
  opens from the latency pill.
- Pairing: sign in to PSN from your phone by scanning a QR code (the Thor
  serves a one-time page on your network), or paste an account ID; then the
  console's 8-digit code. Pairing keys are encrypted with a key in the
  Android Keystore.
- Finds paired consoles on another subnet by asking them directly.

Milestone 8 (ship): tagged releases build a Windows zip, a signed Android
APK, an Arch package and the Nix package in CI; see
[releases](building.md#releases).

Run `tether` with no arguments to open the app: it lists the consoles on your
network, links new ones (sign in once, then type the 8-digit code), wakes and
connects, and has the stream settings. In a session, **Esc** opens the quick
menu, **F11** toggles fullscreen and **F3** the stats overlay.

Video is decoded in hardware when possible (VAAPI on Linux, D3D11VA on
Windows; software otherwise, or if you pick it in Settings) and drawn with
wgpu (Vulkan, or Direct3D 12 on Windows).

The same things work from the command line:

```
tether                       # open the app
tether discover              # consoles on the LAN, and whether they're paired
tether login                 # sign in to PSN once (gets your account ID)
tether pair 192.168.1.20     # pair using the 8-digit code the console shows
tether consoles              # paired consoles
tether wake "Living Room"    # wake a paired console (--wait: until it's ready)
tether stream "Living Room"  # open the app and connect straight away
tether unpair "Living Room"
```

`tether stream` takes `--resolution 360p|540p|720p|1080p`, `--fps 30|60` and
`--codec h264|h265` for that one session; otherwise it uses the app's
settings (720p, 60 fps, H.264 until you change them).

While streaming:

- **Sound** plays on the default output device. `--audio-buffer <ms>` (default
  30) trades delay for robustness on a jittery network; `--no-audio` turns
  it off.
- **Controllers** (anything SDL3 knows: DualSense, DualShock 4, Xbox, Switch
  Pro, ...) drive the console, including the DualSense touchpad and motion
  sensors. Several controllers act as one. Plug them in before or during
  the stream.
- **Rumble** comes back to the controller. Games that use DualSense haptics
  get them as rumble (true haptics are not supported yet); DualSense
  adaptive triggers, light bar and player lights work.
- **Microphone**: the DualSense mute button switches your default
  microphone on and off for voice chat; `--mic` starts with it on.

On Linux, SDL3 needs read access to the controller's `/dev/hidraw*` node for
rumble, triggers, touchpad and motion (Steam's udev rules, or your
distribution's `game-devices-udev` package, set that up).

```
$ tether discover
NAME         ADDRESS       STATE  PAIRED
Living Room  192.168.1.20  ready  yes
```

To pair, turn the console on and open **Settings > System > Remote Play >
Link Device**; it shows the code for a few minutes.

`tether login` opens Sony's sign-in page in your browser. After you sign in
the browser ends on a blank page; paste that page's address back into the
terminal. Only the numeric account ID is kept. If you already use chiaki-ng,
`tether login --account-id <base64>` takes its account ID directly.

Where things are stored:

- `config.toml` (account ID, stream settings, paired consoles): `~/.config/tether/` on Linux,
  `%APPDATA%\tether\config\` on Windows.
- Console keys: the system credential store (Windows Credential Manager, or
  the Secret Service, i.e. gnome-keyring or KWallet, on Linux). Without a
  keyring daemon they go to `secrets.toml` next to `config.toml`, readable
  only by you.
