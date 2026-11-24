# Status

Milestones 1 (discover), 2 (pair) and 3 (first picture). Command line plus a
plain video window so far; no sound or controller input yet:

```
tether discover              # consoles on the LAN, and whether they're paired
tether login                 # sign in to PSN once (gets your account ID)
tether pair 192.168.1.20     # pair using the 8-digit code the console shows
tether consoles              # paired consoles
tether wake "Living Room"    # wake a paired console (--wait: until it's ready)
tether stream "Living Room"  # wake it if needed and show its video in a window
tether unpair "Living Room"
```

`tether stream` takes `--resolution 360p|540p|720p|1080p` (default 720p),
`--fps 30|60` and `--codec h264|h265`. Video is decoded in software for now.

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

- `config.toml` (account ID, paired consoles): `~/.config/tether/` on Linux,
  `%APPDATA%\tether\config\` on Windows.
- Console keys: the system credential store (Windows Credential Manager, or
  the Secret Service, i.e. gnome-keyring or KWallet, on Linux). Without a
  keyring daemon they go to `secrets.toml` next to `config.toml`, readable
  only by you.
