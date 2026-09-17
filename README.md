# Raven Gaming

Gaming setup and capture for Raven Linux, written in Rust with GTK 4 and
libadwaita. There is no JavaScript or embedded browser runtime.

Getting a game running on Linux is rarely one big problem. It is five small
ones that each look like something else: a driver that is installed but
missing its 32-bit half, a kernel update that left a DKMS module behind, a
laptop quietly rendering on the wrong graphics card, a kernel limit tuned
for servers that a large game runs straight into. Raven Gaming finds those,
says what each one actually means, and where the computer can fix it,
offers a button that does.

## What it does

**Gaming** — a banner carrying the one thing worth saying first (is this
machine ready, what is it playing on, how to keep a clip), four quick
controls, the full readiness list, and a shelf of the installed games with
their real Steam cover art. Every line of the list is one thing a game
needs; anything with a Fix button is sorted out from here, and *Fix what is
missing* runs the lot in one pass with one password prompt.

**Graphics drivers** — every card on the PCI bus, which kernel module is
bound to it, and what is missing around it. A DKMS module built for one
kernel and not the one you are about to boot is called out by name, with a
Build button per kernel. Vulkan drivers are listed with whether the library
each manifest points at is actually on disk, and `vkcube` is one click away.

**Performance** — Quiet, Balanced and Performance presets that set the
processor's power profile and, on cards that allow it, hold the graphics
clocks. Under that, the four kernel settings that matter for games —
`vm.max_map_count`, `kernel.split_lock_mitigate`, `vm.swappiness` and the
open-file limit Wine's esync needs — each with what it is now and what it
would become. Applying writes one file in `/etc/sysctl.d` and one in
`/etc/security/limits.d`, both named for this app, so undoing everything is
deleting two files.

**Games** — what Steam, Lutris and Heroic have installed, and the launch
options this particular machine needs, ready to copy. On a hybrid laptop
that is the PRIME offload the discrete card needs; without it a game runs
on the integrated GPU, which works, and is several times slower, and is why
nobody suspects it.

**Capture** — the recordings Huginn has made, how long each runs, and a
button that turns one into an MP4 with `raven-export`.

**Audio** — how far behind the picture the sound is, in milliseconds,
and the buffer size that decides it. PipeWire's default of 1024 frames is
the right answer for a desktop and the wrong one for a game. Changes go to
the running server first so a buffer too small for the machine can be heard
crackling and put back, and are only then written to one file in your own
config — no password, and deleting it restores the defaults. Also which
device the sound goes to, and whether the 32-bit audio libraries a Proton
game needs are installed, since without them the game runs perfectly and in
total silence.

**Controllers** — what is plugged in, over what, on which driver, with what
battery, and whether this session can open its event node — which is
exactly what a game does. Press Test and every button and stick shows up
live; anything that does not light up there will not work in a game either.
Also whether the `steam-devices` udev rules are installed, which is the
usual reason a pad works in Steam and nowhere else.

**Emulators** — sixteen emulators grouped by the console they run, each one
package away, with the three things they all need from the machine checked
first. Where a BIOS, firmware or key dump is required it says so; Raven
Gaming installs emulators and does not supply those.

**Game tools** — the overlays, wrappers and diagnostics, what each one is
actually for, and whether it is installed. Under them, the real state of
this machine's Proton: which builds Steam has, and every game prefix with
its size and the game it belongs to — including the ones left behind by
games since uninstalled. Protontricks, a file-manager button, and a
deletion that is guarded four ways and confirmed once.

**Screen sharing** — what a screen share needs and what this desktop has.
See the note below.

Beside every page, a rail carries the machine's vital signs — CPU, GPU and
memory rings, VRAM and the games drive — the launchers that are installed,
and a line of advice about whatever page is open. The search in the header
looks across sections, checks, cards, settings, games and recordings, and
takes you to whichever one you pick.

## Screen sharing does not work yet, and this app says so

Sharing a screen into a call on Wayland needs three things: an application
asking `xdg-desktop-portal`, a portal backend, and a compositor that will
hand over frames. Huginn implements no capture protocol — it holds the
framebuffer alone and records the screen itself — so a backend has nothing
to ask, and no combination of packages changes that. It needs work in the
compositor.

The Screen sharing page therefore explains the gap instead of offering an
install that would succeed and then fail. Recording is unaffected:
`Super`+`Print` captures at full quality, and the Capture page turns a
recording into a file you can send.

## How it changes things

Reading is free and happens on its own. Writing happens on a click, and
only two ways:

- **Packages** go through `rvn --json`, which hands the root half to `rvnd`
  itself. No sudo, no password dialog — the same path Raven Store uses.
- **Everything else** re-runs this binary as `raven-gaming --apply …` under
  `run0` or `pkexec`. That privileged half matches its arguments against a
  fixed list of four actions and refuses anything else, so a bug in the
  window cannot become an arbitrary write as root.

Audio settings need neither: PipeWire takes them from the running session
and from a file in your own config.

Two things can be deleted, and both are fenced. A shader cache has to
resolve inside your own `XDG_CACHE_HOME`. A Proton prefix has to be named
as a Steam app id, sit directly under a `compatdata` belonging to a Steam
library this machine has, and actually contain a prefix — all four, checked
again after the confirmation, because the dialog has been open in the
meantime and the cost of being wrong is somebody's saves.

## What it reads

| Fact | Where from |
|---|---|
| Graphics cards, vendor and device IDs, bound driver | `/sys/bus/pci/devices` |
| Render nodes and which card drives the laptop's panel | `/sys/class/drm` |
| Card names | `hwdata`'s `pci.ids`, when installed |
| Live AMD and Intel readings | `sysfs` and the card's `hwmon` |
| Live NVIDIA readings | `nvidia-smi`, one call per tick |
| Installed packages | `rvn --json list`, or `/var/lib/pacman/local` |
| Driver modules per kernel | `dkms status` and `/usr/lib/modules` |
| Steam's library | `libraryfolders.vdf` and `appmanifest_*.acf` |
| Launch options already set | `localconfig.vdf` |
| Recordings | the `raven-rec` file header, walked without decoding a frame |
| Screen-sharing backends | the `.portal` files in `/usr/share/xdg-desktop-portal` |
| Memory in use | `/proc/meminfo`, as `MemTotal - MemAvailable` |
| How full the games drive is | `statvfs` on the first Steam library |
| The screen's mode | GDK, which has the compositor's own answer |
| Game covers | `appcache/librarycache`, where Steam already put them |
| Audio latency and devices | `pw-metadata` and `pw-dump`, the tools PipeWire ships |
| Controllers | `/proc/bus/input/devices`, and `/dev/input/eventN` for the tester |
| Controller batteries | `/sys/class/power_supply`, filtered to `scope = Device` |
| Proton builds and prefixes | `steamapps/common`, `compatibilitytools.d`, `compatdata` |

A reading that is not available is drawn as a dash and a check that cannot
be established says so. Nothing reports a state it has not observed.

## Build and run

The system needs Rust, GTK 4 and libadwaita. Everything else — `rvn`,
`dkms`, `nvidia-smi`, `raven-export`, Steam — is detected at runtime and
its absence is reported rather than assumed.

```bash
cargo run
```

For a production build:

```bash
cargo build --release
```

Or with [ImLazy](../RavenLinux):

```bash
imlazy build
imlazy check      # fmt, clippy, and the tests
sudo imlazy install
```

## Appearance

Raven Gaming follows `~/.config/raven/desktop.toml` — theme mode, accent
colour and window transparency — which Raven Settings owns. The shared
`raven-glass.css` sheet is byte-identical to the copy in Raven Settings,
Raven Store and Raven Power; `style.css` is this app's own and layers over
it.

One deliberate departure from the shared palette: the window's base is a
deep blue-black rather than the shared neutral, because everything in it
sits over a painted night scene and a grey base shows through the glass as
a haze. Every *component* — cards, rows, badges, controls, the accent's two
jobs — keeps the shared colours, so a button here is the same button as in
Raven Settings.

The scenery is drawn, not shipped: `art.rs` paints the backdrop, the hero's
sky and ridges, the foot of the sidebar and the gauge rings with a few
hundred lines of Cairo. That weighs nothing in the repository, scales to
any window without a second asset, and takes the accent as a parameter, so
the picture behind "Ready to play" belongs to the person's desktop rather
than to a stock library. Every scene is generated from a fixed seed, so the
same window always draws the same mountains.

Game covers are the exception, and they are not shipped either: they are
read from `appcache/librarycache`, where Steam already put them, for games
this user owns. A game with no cover on disk gets a lettered plate rather
than an empty frame.

Icon names go through a fallback table before they reach a widget. Adwaita
has no thermometer and no `emblem-ok`; Breeze has both. A name the theme
does not carry renders as a grey box, and a window full of grey boxes looks
broken rather than unthemed.
