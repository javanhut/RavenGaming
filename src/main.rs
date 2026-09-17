//! Raven Gaming — one window for everything that stands between a Raven
//! install and a game that runs.
//!
//! The premise is that nobody should have to know what a DKMS module is,
//! that Proton is 32-bit, or why their laptop renders on the wrong card.
//! Each page answers one question in the person's own terms, says plainly
//! what is wrong, and where there is something the computer can do about
//! it, offers a button that does.
//!
//! Three rules the pages keep:
//!
//! * **Never claim a state that has not been observed.** A reading that is
//!   not available is a dash; a check that cannot be established says
//!   "unknown". A gaming app that reports a cheerful green tick it has not
//!   earned is worse than no app.
//! * **Never offer a fix that will not work.** Screen sharing cannot work
//!   on this compositor today, so that page explains why instead of
//!   offering a package that would install and then fail.
//! * **Nothing is changed without being asked.** Reading is free; writing
//!   happens on a click, and the one privileged path is a fixed list of
//!   actions validated on the other side.
//!
//! # The shape of the window
//!
//! ```text
//! ┌──────────┬──────────────────────────────────────┬─────────┐
//! │ brand    │  Page title            search        │ wordmark│
//! │ nav      ├──────────────────────────────────────┼─────────┤
//! │          │                                      │ status  │
//! │          │  the page                            │ actions │
//! │ ridge    │                                      │ tip     │
//! └──────────┴──────────────────────────────────────┴─────────┘
//! ```
//!
//! A painted backdrop fills the whole window and everything else is glass
//! over it. The header is a drag handle rather than a title bar, so the
//! page's own title is the only title; the window controls float over the
//! top-right corner. The rail on the right is outside the page stack — the
//! machine's vital signs are worth having on every page, not just one.

mod art;
mod audio;
mod capture;
mod checks;
mod controllers;
mod desktop;
mod drivers;
mod emulators;
mod games;
mod gpu;
mod install;
mod share;
mod telemetry;
mod tools;
mod tune;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};

use art::Rgb;
use checks::{Fix, State};
use gpu::Gpu;
use telemetry::{CpuReading, GpuReading, Usage};

const APP_ID: &str = "org.raven.Gaming";

/// How often the live readings are taken. Two seconds: fast enough that a
/// game launching visibly moves the numbers, slow enough that reading them
/// — which on NVIDIA means running `nvidia-smi` — costs nothing worth
/// measuring.
const TICK: Duration = Duration::from_secs(2);

/// How often the main loop looks for a reading the background thread has
/// left for it.
const POLL: Duration = Duration::from_millis(250);

/// The rail goes away before the sidebar does: losing the vital signs is a
/// smaller loss than losing the way between pages.
const RAIL_BREAKPOINT: f64 = 1180.0;
const SIDEBAR_BREAKPOINT: f64 = 900.0;

/// One section. Stack name, sidebar glyph, sidebar label, page title, and
/// the line under the title.
struct Page {
    name: &'static str,
    icon: &'static str,
    label: &'static str,
    title: &'static str,
    lede: &'static str,
    /// The contextual line in the rail's tip card.
    tip: &'static str,
}

const PAGES: [Page; 10] = [
    Page {
        name: "overview",
        icon: "view-grid-symbolic",
        label: "Gaming",
        title: "Gaming",
        lede: "Set it up once. Then just play.",
        tip: "Anything with a Fix button can be sorted out without leaving this window.",
    },
    Page {
        name: "graphics",
        icon: "video-display-symbolic",
        label: "Graphics",
        title: "Graphics drivers",
        lede: "Your cards, the drivers on them, and the modules for every kernel.",
        tip: "A driver built with DKMS is compiled per kernel. Build for a new one before you boot it, not after.",
    },
    Page {
        name: "performance",
        icon: "power-profile-performance-symbolic",
        label: "Performance",
        title: "Performance",
        lede: "How hard the machine may work, and the settings games depend on.",
        tip: "The memory-mapping limit is the one that stops a big game dead. Everything else here costs smoothness, not startup.",
    },
    Page {
        name: "audio",
        icon: "audio-speakers-symbolic",
        label: "Audio",
        title: "Audio",
        lede: "Where the sound goes, and how far behind the picture it is.",
        tip: "A smaller buffer means the bang arrives with the muzzle flash. Too small and it crackles — the change is applied live so you can hear which.",
    },
    Page {
        name: "controllers",
        icon: "input-gaming-symbolic",
        label: "Controllers",
        title: "Controllers",
        lede: "What is plugged in, and whether every button on it works.",
        tip: "A pad that works in Steam and nowhere else is usually the steam-devices udev rules missing, not the pad.",
    },
    Page {
        name: "games",
        icon: "applications-games-symbolic",
        label: "Library",
        title: "Library",
        lede: "What is installed, and how each one should be started.",
        tip: "On a laptop with two cards, a game with no launch options runs on the slow one. Paste the line at the top into Steam.",
    },
    Page {
        name: "emulators",
        icon: "applications-system-symbolic",
        label: "Emulators",
        title: "Emulators",
        lede: "The consoles this machine can play, and what each one needs.",
        tip: "Most emulators want a controller mapped before they accept any input at all. Several want a BIOS you have to dump yourself.",
    },
    Page {
        name: "tools",
        icon: "applications-utilities-symbolic",
        label: "Game tools",
        title: "Game tools",
        lede: "Overlays, wrappers, diagnostics, and the Proton prefixes they act on.",
        tip: "Deleting a game's prefix and letting Proton rebuild it is the oldest fix there is. Saves usually live in the prefix too, so copy them out first.",
    },
    Page {
        name: "capture",
        icon: "media-record-symbolic",
        label: "Capture",
        title: "Capture",
        lede: "Recording gameplay, and turning a recording into a video.",
        tip: "Super+Print starts and stops a recording. The compositor draws it, so a busy game cannot make it drop frames.",
    },
    Page {
        name: "sharing",
        icon: "network-transmit-receive-symbolic",
        label: "Screen sharing",
        title: "Screen sharing",
        lede: "What a screen share needs, and what this desktop can do today.",
        tip: "Sharing into a call needs capture support in the compositor. Recording does not, and works now.",
    },
];

fn page(name: &str) -> &'static Page {
    PAGES.iter().find(|p| p.name == name).unwrap_or(&PAGES[0])
}

// ---- icons ---------------------------------------------------------------

/// Icon names this app asks for that are not in every theme, and what to
/// use instead.
///
/// Adwaita has no thermometer and no checkmark called `emblem-ok`; Breeze
/// has both. An icon a theme does not carry renders as a grey box, and a
/// window full of grey boxes looks broken rather than unthemed — so every
/// name goes through here and comes out as one the theme actually has.
const ICON_FALLBACKS: [(&str, &[&str]); 7] = [
    (
        "emblem-ok-symbolic",
        &["object-select-symbolic", "checkbox-checked-symbolic"],
    ),
    (
        "temperature-symbolic",
        &["weather-clear-symbolic", "display-brightness-symbolic"],
    ),
    (
        "utilities-system-monitor-symbolic",
        &["power-profile-performance-symbolic", "system-run-symbolic"],
    ),
    ("steam", &["applications-games-symbolic"]),
    (
        "net.lutris.Lutris",
        &["lutris", "applications-games-symbolic"],
    ),
    (
        "com.heroicgameslauncher.hgl",
        &["heroic", "applications-games-symbolic"],
    ),
    ("raven-logo", &["input-gaming-symbolic"]),
];

/// The last resort, which every theme has because GTK ships it.
const ICON_LAST_RESORT: &str = "application-x-executable-symbolic";

fn icon_name(wanted: &str) -> String {
    let Some(display) = gdk::Display::default() else {
        return wanted.to_string();
    };
    let theme = gtk::IconTheme::for_display(&display);
    if theme.has_icon(wanted) {
        return wanted.to_string();
    }
    ICON_FALLBACKS
        .iter()
        .find(|(name, _)| *name == wanted)
        .and_then(|(_, alternatives)| {
            alternatives
                .iter()
                .find(|name| theme.has_icon(name))
                .map(|name| name.to_string())
        })
        .unwrap_or_else(|| ICON_LAST_RESORT.to_string())
}

/// Every icon in this window is made here, so none of them can be a box.
fn glyph(wanted: &str) -> gtk::Image {
    gtk::Image::from_icon_name(&icon_name(wanted))
}

// ---- live readings -------------------------------------------------------

/// One round of live readings, taken off the main thread.
#[derive(Debug, Clone, Default)]
pub struct Readings {
    /// One per card, in the order the cards were discovered.
    pub gpus: Vec<GpuReading>,
    pub cpu: CpuReading,
    pub memory: Usage,
    pub storage: Usage,
    /// Whether the compositor looks to be recording right now.
    pub recording: bool,
}

/// One widget's reaction to a fresh round of readings.
type Updater = Box<dyn Fn(&Readings)>;

/// Every widget that follows the readings: a closure each, run after every
/// round. The same shape Raven Power uses, for the same reason — the
/// alternative is threading a dozen widget handles through every page
/// builder.
#[derive(Default)]
struct Live {
    updaters: RefCell<Vec<Updater>>,
}

impl Live {
    fn bind(&self, update: impl Fn(&Readings) + 'static) {
        self.updaters.borrow_mut().push(Box::new(update));
    }

    fn label(&self, label: &gtk::Label, text: impl Fn(&Readings) -> String + 'static) {
        let label = label.clone();
        self.bind(move |r| label.set_text(&text(r)));
    }

    /// Dropped when the pages holding these widgets are rebuilt; otherwise
    /// every rebuild would leave its predecessors updating widgets that
    /// are no longer on screen.
    fn clear(&self) {
        self.updaters.borrow_mut().clear();
    }

    fn run(&self, readings: &Readings) {
        for update in self.updaters.borrow().iter() {
            update(readings);
        }
    }
}

// ---- the application -----------------------------------------------------

struct App {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    /// The rail is rebuilt with the pages, so it holds the same fresh view
    /// of the system they do.
    rail: gtk::Box,
    heading: gtk::Label,
    lede: gtk::Label,
    /// The one part of the rail that follows the page.
    tip: gtk::Label,
    live: Rc<Live>,
    /// Everything read from the system, replaced whole on a refresh.
    system: RefCell<Rc<checks::System>>,
    /// The most recent live readings, so a page built mid-session starts
    /// with numbers rather than dashes.
    latest: RefCell<Readings>,
    /// The person's accent, read once; every painting takes it.
    accent: Rgb,
    /// True while a transaction is running, so two cannot overlap.
    busy: Cell<bool>,
}

impl App {
    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    fn system(&self) -> Rc<checks::System> {
        self.system.borrow().clone()
    }

    fn show_page(self: &Rc<Self>, name: &str) {
        self.stack.set_visible_child_name(name);
    }

    /// Re-reads the system and redraws every page. Called after anything
    /// that changes what the pages are reporting.
    fn refresh(self: &Rc<Self>) {
        *self.system.borrow_mut() = Rc::new(checks::System::read());
        self.rebuild_pages();
    }

    fn rebuild_pages(self: &Rc<Self>) {
        let showing = self
            .stack
            .visible_child_name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| PAGES[0].name.to_string());
        self.live.clear();
        for page in &PAGES {
            if let Some(old) = self.stack.child_by_name(page.name) {
                self.stack.remove(&old);
            }
            self.stack
                .add_named(&build_page(self, page.name), Some(page.name));
        }
        self.stack.set_visible_child_name(&showing);
        fill_rail(self);
        self.live.run(&self.latest.borrow());
    }
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // The privileged half. Reached only by this binary re-running itself
    // under run0 or pkexec; it validates its own arguments and does one
    // thing from a fixed list.
    if args.get(1).map(String::as_str) == Some("--apply") {
        return match tune::apply_privileged(&args[2..]) {
            Ok(()) => glib::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("raven-gaming: {error}");
                // When this half is running in a terminal the window
                // closes the instant it exits, taking the only
                // explanation with it. Holding here keeps the message on
                // screen; with no terminal attached there is nobody to
                // wait for and it exits at once.
                if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    eprintln!("\nPress Enter to close this window.");
                    let mut discard = String::new();
                    let _ = std::io::stdin().read_line(&mut discard);
                }
                glib::ExitCode::FAILURE
            }
        };
    }
    if matches!(args.get(1).map(String::as_str), Some("--help" | "-h")) {
        println!(
            "Raven Gaming — gaming setup and capture for Raven Linux\n\n\
             usage: raven-gaming\n\n\
             Run with no arguments to open the window."
        );
        return glib::ExitCode::SUCCESS;
    }

    // `NON_UNIQUE`: a second launch is a second window, as for every Raven
    // app, rather than a raise of the first.
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_ui);
    app.run_with_args::<&str>(&[])
}

/// The shared Raven Glass sheet, then this app's own classes, in one
/// provider; the accent and light-mode overrides go in a second one above
/// it, exactly as Settings, Store and Power layer theirs.
fn load_css() {
    let display = gdk::Display::default().expect("A graphical display is required");
    let provider = gtk::CssProvider::new();
    provider.load_from_string(concat!(
        include_str!("raven-glass.css"),
        include_str!("style.css")
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let desktop = desktop::Desktop::load();
    let look = &desktop.appearance;
    adw::StyleManager::default().set_color_scheme(match look.theme_mode {
        desktop::ThemeMode::Dark => adw::ColorScheme::ForceDark,
        desktop::ThemeMode::Light => adw::ColorScheme::ForceLight,
        desktop::ThemeMode::Auto => adw::ColorScheme::PreferDark,
    });
    let accent = desktop.accent();
    let mut css =
        format!("@define-color accent_bg_color {accent};\n@define-color accent_color {accent};\n");
    if look.theme_mode == desktop::ThemeMode::Light {
        css.push_str(include_str!("raven-glass-light.css"));
    }
    let overrides = gtk::CssProvider::new();
    overrides.load_from_string(&css);
    gtk::style_context_add_provider_for_display(
        &display,
        &overrides,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}

fn build_ui(application: &adw::Application) {
    let desktop = desktop::Desktop::load();
    let accent = Rgb::from_hex(desktop.accent());
    let glass = desktop.appearance.transparency;

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Raven Gaming")
        .default_width(1400)
        .default_height(920)
        .build();
    window.add_css_class("raven");
    // Alpha only; the blur behind a glass window is the compositor's.
    if glass {
        window.add_css_class("glass");
    }

    let app = Rc::new(App {
        toasts: adw::ToastOverlay::new(),
        // Not homogeneous: a stack that sizes itself to its widest page
        // makes every other page's window as wide as the worst one, and
        // the window cannot then be made smaller than a page nobody is
        // looking at.
        stack: gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .hhomogeneous(false)
            .vhomogeneous(false)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build(),
        rail: gtk::Box::new(gtk::Orientation::Vertical, 14),
        heading: gtk::Label::new(Some(PAGES[0].title)),
        lede: gtk::Label::new(Some(PAGES[0].lede)),
        tip: gtk::Label::new(Some(PAGES[0].tip)),
        live: Rc::new(Live::default()),
        system: RefCell::new(Rc::new(checks::System::read())),
        latest: RefCell::new(Readings::default()),
        accent,
        busy: Cell::new(false),
        window: window.clone(),
    });

    // The pages go in before the sidebar, which selects its first row as
    // it is built — and a selection is a request to show a page, which
    // has to be there to be shown.
    for page in &PAGES {
        app.stack
            .add_named(&build_page(&app, page.name), Some(page.name));
    }
    let navigation = build_sidebar(&app);
    let content = build_content(&app);

    let split = adw::OverlaySplitView::builder()
        .sidebar(&navigation.0)
        .content(&content)
        .sidebar_width_fraction(0.19)
        .min_sidebar_width(238.0)
        .max_sidebar_width(272.0)
        .build();

    // The painted night behind everything. Its alpha follows the desktop's
    // transparency setting: opaque on its own, and letting the compositor's
    // blur through when the desktop asks for glass.
    let backdrop = gtk::DrawingArea::new();
    backdrop.set_draw_func(move |_, cr, width, height| {
        if glass {
            cr.push_group();
        }
        art::backdrop(cr, width as f64, height as f64, accent);
        if glass {
            let _ = cr.pop_group_to_source();
            let _ = cr.paint_with_alpha(0.86);
        }
    });

    let root = gtk::Overlay::new();
    root.add_css_class("app-root");
    root.set_overflow(gtk::Overflow::Hidden);
    root.set_child(Some(&backdrop));
    root.add_overlay(&split);

    // The window buttons float over the top-right corner rather than
    // living in a title bar, because the page's own heading is the title
    // and a second one above it would be saying the same thing twice.
    let controls = gtk::WindowControls::new(gtk::PackType::End);
    controls.add_css_class("window-controls");
    controls.set_halign(gtk::Align::End);
    controls.set_valign(gtk::Align::Start);
    root.add_overlay(&controls);

    app.toasts.set_child(Some(&root));
    window.set_content(Some(&app.toasts));

    // Two breakpoints, in the order things become expendable.
    let (sidebar_toggle, rail_holder) = (navigation.1, content_rail_holder(&content));
    let narrow_rail = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        RAIL_BREAKPOINT,
        adw::LengthUnit::Px,
    ));
    narrow_rail.add_setter(&rail_holder, "visible", Some(&false.to_value()));
    window.add_breakpoint(narrow_rail);

    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        SIDEBAR_BREAKPOINT,
        adw::LengthUnit::Px,
    ));
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    narrow.add_setter(&sidebar_toggle, "visible", Some(&true.to_value()));
    window.add_breakpoint(narrow);
    split
        .bind_property("show-sidebar", &sidebar_toggle, "active")
        .bidirectional()
        .sync_create()
        .build();

    fill_rail(&app);

    window.set_size_request(560, 420);
    window.present();
    start_readings(&app);
}

/// Takes readings on a background thread and applies them on the main one.
///
/// Off the main thread because an NVIDIA reading means running
/// `nvidia-smi`, and spawning a process on the frame clock is how a window
/// starts to feel sticky. The thread stops when the window goes away.
fn start_readings(app: &Rc<App>) {
    let gpus: Vec<Gpu> = app.system().gpus.clone();
    // The drive the games are on, which is the one worth a gauge — a full
    // root partition is a different problem from a full games library.
    let storage_path = games::library_root();
    let stop = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel::<Readings>();
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut cpu_totals = telemetry::CpuTotals::default();
            loop {
                let (cpu, totals) = telemetry::read_cpu(cpu_totals);
                cpu_totals = totals;
                let readings = Readings {
                    gpus: gpus.iter().map(telemetry::read_gpu).collect(),
                    cpu,
                    memory: telemetry::read_memory(),
                    storage: telemetry::read_storage(&storage_path),
                    recording: share::recording_in_progress(),
                };
                if sender.send(readings).is_err() {
                    return;
                }
                // Woken in short steps so closing the window does not wait
                // out a whole tick before the thread notices.
                for _ in 0..(TICK.as_millis() / 100).max(1) {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
    }
    app.window.connect_close_request(glib::clone!(
        #[strong]
        stop,
        move |_| {
            stop.store(true, Ordering::Relaxed);
            glib::Propagation::Proceed
        }
    ));

    let weak_window = app.window.downgrade();
    glib::timeout_add_local(
        POLL,
        glib::clone!(
            #[strong]
            app,
            move || {
                if weak_window.upgrade().is_none() {
                    stop.store(true, Ordering::Relaxed);
                    return glib::ControlFlow::Break;
                }
                // Drain to the newest: if the main loop was busy, the
                // stale rounds behind it are of no interest.
                let mut newest = None;
                loop {
                    match receiver.try_recv() {
                        Ok(readings) => newest = Some(readings),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                    }
                }
                if let Some(readings) = newest {
                    app.live.run(&readings);
                    *app.latest.borrow_mut() = readings;
                }
                glib::ControlFlow::Continue
            }
        ),
    );
}

/// The sidebar, and the button that brings it back when it has collapsed.
fn build_sidebar(app: &Rc<App>) -> (gtk::Widget, gtk::ToggleButton) {
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("sidebar");

    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    brand.add_css_class("brand");
    brand.append(&glyph("raven-logo"));
    let brand_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let name = gtk::Label::new(Some("Raven Linux"));
    name.set_xalign(0.0);
    name.add_css_class("app-title");
    brand_text.append(&name);
    let tagline = gtk::Label::new(Some("Play without limits"));
    tagline.set_xalign(0.0);
    tagline.add_css_class("app-subtitle");
    brand_text.append(&tagline);
    brand.append(&brand_text);
    sidebar.append(&brand);

    let navigation = gtk::ListBox::new();
    navigation.add_css_class("navigation-sidebar");
    navigation.set_selection_mode(gtk::SelectionMode::Single);
    for page in &PAGES {
        navigation.append(&nav_row(page.icon, page.label));
    }
    sidebar.append(&navigation);

    // The ridge at the foot, with the line sitting on it.
    let footer = gtk::Overlay::new();
    footer.set_vexpand(true);
    footer.set_valign(gtk::Align::End);
    let ridge = gtk::DrawingArea::new();
    ridge.set_content_height(168);
    let accent = app.accent;
    ridge.set_draw_func(move |_, cr, width, height| {
        art::sidebar_footer(cr, width as f64, height as f64, accent);
    });
    footer.set_child(Some(&ridge));
    let note_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    note_box.set_valign(gtk::Align::End);
    let note = gtk::Label::new(Some("Games feel better here."));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("sidebar-note");
    note_box.append(&note);
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("sidebar-rule");
    rule.set_halign(gtk::Align::Start);
    note_box.append(&rule);
    footer.add_overlay(&note_box);
    sidebar.append(&footer);

    let toggle = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Sections")
        .visible(false)
        .valign(gtk::Align::Center)
        .build();
    toggle.add_css_class("flat");

    navigation.connect_row_selected(glib::clone!(
        #[strong]
        app,
        move |_, row| {
            let Some(row) = row else { return };
            let Some(page) = PAGES.get(row.index() as usize) else {
                return;
            };
            app.show_page(page.name);
            app.heading.set_text(page.title);
            app.lede.set_text(page.lede);
            app.tip.set_text(page.tip);
        }
    ));
    navigation.select_row(navigation.row_at_index(0).as_ref());

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(false)
        .child(&sidebar)
        .build();
    scroller.add_css_class("sidebar-pane");
    (scroller.upcast(), toggle)
}

fn nav_row(icon: &str, label: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 13);
    let glyph = glyph(icon);
    glyph.add_css_class("nav-glyph");
    content.append(&glyph);
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    content.append(&text);
    row.set_child(Some(&content));
    row
}

/// The header strip and the two columns under it.
fn build_content(app: &Rc<App>) -> gtk::Widget {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // ---- header ----
    let header = gtk::CenterBox::new();
    header.add_css_class("page-header");
    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
    title_box.set_valign(gtk::Align::Center);
    app.heading.set_xalign(0.0);
    app.heading.set_wrap(true);
    app.heading.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    app.heading.add_css_class("page-heading");
    title_box.append(&app.heading);
    app.lede.set_xalign(0.0);
    app.lede.set_wrap(true);
    app.lede.add_css_class("page-lede");
    title_box.append(&app.lede);
    title_box.set_margin_end(22);
    header.set_start_widget(Some(&title_box));

    let search = build_search(app);
    search.set_halign(gtk::Align::Center);
    search.set_valign(gtk::Align::Center);
    search.set_size_request(340, -1);
    header.set_center_widget(Some(&search));

    let wordmark = gtk::Box::new(gtk::Orientation::Vertical, 1);
    wordmark.set_valign(gtk::Align::Center);
    wordmark.set_halign(gtk::Align::End);
    let mark_name = gtk::Label::new(Some("Raven Linux"));
    mark_name.set_xalign(1.0);
    mark_name.add_css_class("wordmark-name");
    wordmark.append(&mark_name);
    let mark_line = gtk::Label::new(Some("Built for players"));
    mark_line.set_xalign(1.0);
    mark_line.add_css_class("wordmark-line");
    wordmark.append(&mark_line);
    // Room for the window buttons, which float over this corner.
    wordmark.set_margin_end(104);
    wordmark.set_margin_start(22);
    header.set_end_widget(Some(&wordmark));

    // The header doubles as the drag handle: with no title bar there is
    // nowhere else to take hold of the window.
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&header));
    column.append(&handle);

    // ---- pages and rail ----
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&app.stack);

    let rail_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&app.rail)
        .build();
    rail_scroller.add_css_class("rail-scroll");
    rail_scroller.set_size_request(296, -1);
    // A size request is a floor, not a width. Without this the rail takes
    // its share of every spare pixel and the page it sits beside — the
    // part anyone is actually reading — ends up the narrower of the two.
    rail_scroller.set_hexpand(false);
    app.rail.set_hexpand(false);
    app.rail.add_css_class("rail");
    body.append(&rail_scroller);
    column.append(&body);
    column.upcast()
}

/// The scroller holding the rail, which the breakpoint hides.
fn content_rail_holder(content: &gtk::Widget) -> gtk::Widget {
    // column → body → [stack, rail scroller]
    content
        .last_child()
        .and_then(|body| body.last_child())
        .unwrap_or_else(|| content.clone())
}

// ---- search --------------------------------------------------------------

/// One thing the search can take you to.
struct Hit {
    page: &'static str,
    where_: &'static str,
    title: String,
    detail: String,
    icon: &'static str,
}

/// Everything worth finding by name: the sections, each readiness check,
/// each installed game, each system setting, and each card.
///
/// Built from the same structures the pages are built from, so a search
/// result can never name something the page does not show.
fn search_index(system: &checks::System) -> Vec<Hit> {
    let mut hits = Vec::new();
    for p in &PAGES {
        hits.push(Hit {
            page: p.name,
            where_: "Section",
            title: p.title.to_string(),
            detail: p.lede.to_string(),
            icon: p.icon,
        });
    }
    for check in checks::all(system) {
        hits.push(Hit {
            page: "overview",
            where_: "Readiness",
            title: check.title.clone(),
            detail: check.detail.lines().next().unwrap_or_default().to_string(),
            icon: check.state.icon(),
        });
    }
    for gpu in &system.gpus {
        hits.push(Hit {
            page: "graphics",
            where_: "Graphics",
            title: gpu.title(),
            detail: format!("{} · {}", gpu.vendor, gpu.driver.label()),
            icon: "video-display-symbolic",
        });
    }
    for tweak in tune::TWEAKS {
        hits.push(Hit {
            page: "performance",
            where_: "Setting",
            title: tweak.title().to_string(),
            detail: tweak.change_text(),
            icon: "preferences-system-symbolic",
        });
    }
    for game in games::discover() {
        hits.push(Hit {
            page: "games",
            where_: game.source.name(),
            title: game.name.clone(),
            detail: game.detail(),
            icon: "applications-games-symbolic",
        });
    }
    for emulator in emulators::CATALOGUE {
        hits.push(Hit {
            page: "emulators",
            where_: "Emulator",
            title: emulator.name.to_string(),
            detail: emulator.systems.to_string(),
            icon: "applications-system-symbolic",
        });
    }
    for tool in tools::CATALOGUE {
        hits.push(Hit {
            page: "tools",
            where_: tool.kind.title(),
            title: tool.name.to_string(),
            detail: tool.what.to_string(),
            icon: "applications-utilities-symbolic",
        });
    }
    for controller in controllers::discover() {
        hits.push(Hit {
            page: "controllers",
            where_: controller.kind(),
            title: controller.name.clone(),
            detail: controller.transport.name().to_string(),
            icon: "input-gaming-symbolic",
        });
    }
    for device in audio::devices() {
        hits.push(Hit {
            page: "audio",
            where_: if device.is_output { "Output" } else { "Input" },
            title: device.description.clone(),
            detail: device.api.clone(),
            icon: "audio-speakers-symbolic",
        });
    }
    for recording in capture::recordings() {
        hits.push(Hit {
            page: "capture",
            where_: "Recording",
            title: recording.name(),
            detail: recording.detail(),
            icon: "media-record-symbolic",
        });
    }
    hits
}

/// Case-insensitive substring, over the title first and the detail second,
/// so typing "nvidia" puts the card above a sentence that mentions it.
fn rank(hit: &Hit, needle: &str) -> Option<u8> {
    let title = hit.title.to_lowercase();
    if title.starts_with(needle) {
        Some(0)
    } else if title.contains(needle) {
        Some(1)
    } else if hit.detail.to_lowercase().contains(needle) {
        Some(2)
    } else {
        None
    }
}

const MAX_HITS: usize = 8;

fn matches(index: &[Hit], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u8, usize)> = index
        .iter()
        .enumerate()
        .filter_map(|(i, hit)| rank(hit, &needle).map(|score| (score, i)))
        .collect();
    scored.sort_by_key(|(score, i)| (*score, *i));
    scored.into_iter().take(MAX_HITS).map(|(_, i)| i).collect()
}

fn build_search(app: &Rc<App>) -> gtk::Widget {
    let entry = gtk::SearchEntry::new();
    entry.add_css_class("search-field");
    entry.set_placeholder_text(Some("Search games, settings, or tools…"));

    let results = gtk::ListBox::new();
    results.set_selection_mode(gtk::SelectionMode::None);
    let popover = gtk::Popover::new();
    popover.set_parent(&entry);
    popover.set_autohide(false);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_size_request(380, -1);
    popover.set_child(Some(&results));

    let index: Rc<RefCell<Vec<Hit>>> = Rc::new(RefCell::new(Vec::new()));
    entry.connect_search_changed(glib::clone!(
        #[strong]
        app,
        #[strong]
        index,
        #[weak]
        results,
        #[weak]
        popover,
        move |entry| {
            let query = entry.text().to_string();
            if query.trim().is_empty() {
                popover.popdown();
                return;
            }
            // Built on the first keystroke of a search rather than on
            // every refresh: it walks the Steam library and the
            // recordings folder, which is not work to do for a window
            // nobody is searching in.
            if index.borrow().is_empty() {
                *index.borrow_mut() = search_index(&app.system());
            }
            let index = index.borrow();
            let found = matches(&index, &query);
            while let Some(child) = results.first_child() {
                results.remove(&child);
            }
            if found.is_empty() {
                let row = gtk::ListBoxRow::new();
                row.set_activatable(false);
                let label = gtk::Label::new(Some(&format!("Nothing matches “{}”", query.trim())));
                label.set_xalign(0.0);
                label.set_margin_top(10);
                label.set_margin_bottom(10);
                label.set_margin_start(12);
                label.add_css_class("dim-label");
                row.set_child(Some(&label));
                results.append(&row);
            }
            for i in found {
                results.append(&search_row(&app, &index[i], &entry.clone(), &popover));
            }
            popover.popup();
        }
    ));
    entry.connect_stop_search(glib::clone!(
        #[weak]
        popover,
        move |entry| {
            entry.set_text("");
            popover.popdown();
        }
    ));
    entry.upcast()
}

fn search_row(
    app: &Rc<App>,
    hit: &Hit,
    entry: &gtk::SearchEntry,
    popover: &gtk::Popover,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 11);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(10);
    content.set_margin_end(10);
    let icon = glyph(hit.icon);
    icon.set_valign(gtk::Align::Center);
    content.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&hit.title));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.add_css_class("row-title");
    text.append(&title);
    let detail = gtk::Label::new(Some(&format!("{} · {}", hit.where_, hit.detail)));
    detail.set_xalign(0.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
    detail.add_css_class("dim-label");
    text.append(&detail);
    content.append(&text);
    row.set_child(Some(&content));

    let target = hit.page;
    let gesture = gtk::GestureClick::new();
    gesture.connect_released(glib::clone!(
        #[strong]
        app,
        #[weak]
        entry,
        #[weak]
        popover,
        move |_, _, _, _| {
            popover.popdown();
            entry.set_text("");
            jump_to(&app, target);
        }
    ));
    row.add_controller(gesture);
    row
}

/// Shows a page and moves the sidebar selection with it, so the two never
/// disagree about where you are.
fn jump_to(app: &Rc<App>, name: &str) {
    app.show_page(name);
    let page = page(name);
    app.heading.set_text(page.title);
    app.lede.set_text(page.lede);
    app.tip.set_text(page.tip);
    if let Some(index) = PAGES.iter().position(|p| p.name == name)
        && let Some(list) = sidebar_list(app)
    {
        list.select_row(list.row_at_index(index as i32).as_ref());
    }
}

/// The navigation list, found from the window rather than held, because it
/// belongs to the sidebar and only the jump needs it.
fn sidebar_list(app: &Rc<App>) -> Option<gtk::ListBox> {
    fn find(widget: &gtk::Widget) -> Option<gtk::ListBox> {
        if let Some(list) = widget.downcast_ref::<gtk::ListBox>()
            && list.has_css_class("navigation-sidebar")
        {
            return Some(list.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = find(&current) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }
    app.window.content().and_then(|root| find(&root))
}

fn build_page(app: &Rc<App>, name: &str) -> gtk::Widget {
    match name {
        "overview" => overview_page(app),
        "graphics" => graphics_page(app),
        "performance" => performance_page(app),
        "games" => games_page(app),
        "audio" => audio_page(app),
        "controllers" => controllers_page(app),
        "emulators" => emulators_page(app),
        "tools" => tools_page(app),
        "capture" => capture_page(app),
        "sharing" => sharing_page(app),
        other => unreachable!("no such page: {other}"),
    }
}
// ---- shared page furniture ----------------------------------------------

fn page_scroll(content: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(content)
        .build();
    scroll.add_css_class("page-scroll");
    scroll.upcast()
}

fn page_box() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.add_css_class("page");
    page
}

fn section_title(title: &str, subtitle: &str) -> gtk::Box {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.set_wrap(true);
    // Headings here are sometimes a card's full marketing name — "Radeon
    // Vega Series / Radeon Vega Mobile Series" — and a heading that will
    // not wrap makes its own length the page's minimum width.
    heading.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    heading.add_css_class("section-title");
    holder.append(&heading);
    if !subtitle.is_empty() {
        let sub = gtk::Label::new(Some(subtitle));
        sub.set_wrap(true);
        sub.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        sub.set_xalign(0.0);
        sub.add_css_class("dim-label");
        holder.append(&sub);
    }
    holder
}

/// A row of equal cards that wraps onto another line when the window is
/// too narrow for them. A plain homogeneous box would instead refuse to
/// shrink, which is how a settings window ends up with a minimum width
/// nobody chose.
fn card_row(per_line: u32) -> gtk::FlowBox {
    let row = gtk::FlowBox::new();
    row.set_selection_mode(gtk::SelectionMode::None);
    row.set_homogeneous(true);
    row.set_min_children_per_line(1);
    row.set_max_children_per_line(per_line);
    row.set_column_spacing(14);
    row.set_row_spacing(14);
    row
}

fn card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.add_css_class("card");
    card
}

/// A caption over a value; the value label comes back for live binding.
fn stat(caption: &str) -> (gtk::Box, gtk::Label) {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 3);
    holder.set_hexpand(true);
    let label = gtk::Label::new(Some(caption));
    label.set_xalign(0.0);
    label.add_css_class("eyebrow");
    holder.append(&label);
    let value = gtk::Label::new(Some("—"));
    value.set_xalign(0.0);
    value.add_css_class("stat-value");
    holder.append(&value);
    (holder, value)
}

/// A `Super`+`Print`-style key rendering.
fn keys(chord: &[&str]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_valign(gtk::Align::Center);
    for (index, key) in chord.iter().enumerate() {
        if index > 0 {
            let plus = gtk::Label::new(Some("+"));
            plus.add_css_class("dim");
            row.append(&plus);
        }
        let label = gtk::Label::new(Some(key));
        label.add_css_class("kbd");
        row.append(&label);
    }
    row
}

/// A monospace line with a button that copies it. The one interaction
/// this app's advice needs: nobody should be retyping four environment
/// variables from a screen.
fn copy_row(app: &Rc<App>, text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("command-row");
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_selectable(true);
    label.add_css_class("mono");
    row.append(&label);
    let button = gtk::Button::from_icon_name(&icon_name("edit-copy-symbolic"));
    button.set_tooltip_text(Some("Copy"));
    button.set_valign(gtk::Align::Start);
    button.add_css_class("flat");
    let copied = text.to_string();
    button.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |button| {
            button.clipboard().set_text(&copied);
            app.toast("Copied");
        }
    ));
    row.append(&button);
    row
}

/// One line of a list, with an icon in the state's colour, a title, a
/// wrapping detail, and optionally something on the right.
fn check_row(state: State, title: &str, detail: &str, trailing: Option<&gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");
    let icon = glyph(state.icon());
    icon.set_valign(gtk::Align::Start);
    icon.add_css_class(state.css_class());
    icon.set_pixel_size(18);
    row.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.set_wrap(true);
    // A row's title is sometimes a path or a package name — one long word
    // that `Word` wrapping cannot break, and whose full width would become
    // the page's minimum.
    heading.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    heading.add_css_class("row-title");
    text.append(&heading);
    let body = gtk::Label::new(Some(detail));
    body.set_xalign(0.0);
    body.set_wrap(true);
    body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    body.add_css_class("dim-label");
    text.append(&body);
    row.append(&text);
    if let Some(widget) = trailing {
        widget.set_valign(gtk::Align::Center);
        row.append(widget);
    }
    row
}

/// The button a failing check offers, if the app can do anything.
fn fix_button(app: &Rc<App>, fix: &Fix) -> Option<gtk::Widget> {
    match fix {
        Fix::Manual(_) => None,
        fix => {
            let button = gtk::Button::with_label(match fix {
                Fix::Install(names) if names.len() == 1 => "Install",
                Fix::Install(_) => "Install all",
                Fix::BuildModules(_) => "Build",
                _ => "Apply",
            });
            button.add_css_class("pill");
            button.add_css_class("suggested-action");
            let fix = fix.clone();
            button.connect_clicked(glib::clone!(
                #[strong]
                app,
                move |_| run_fixes(&app, vec![fix.clone()])
            ));
            Some(button.upcast())
        }
    }
}

fn empty_state(icon: &str, title: &str, description: &str) -> gtk::Widget {
    let page = adw::StatusPage::new();
    page.set_icon_name(Some(icon));
    page.set_title(title);
    page.set_description(Some(description));
    page.set_vexpand(true);
    page.upcast()
}

// ---- the rail ------------------------------------------------------------

/// Fills the right-hand rail: the machine's vital signs, the launchers
/// that are installed, and a line of advice about the page being looked at.
///
/// Outside the page stack on purpose. Every page here is about making a
/// game run well, and "is anything on fire right now" is worth answering
/// while you are reading any of them.
///
/// Rebuilt only when the system is re-read, never on a page change. The
/// gauges bind themselves to the live readings when they are built, so
/// rebuilding them to change one line of advice would leave a closure
/// behind for every page anyone had visited, and show three dashes until
/// the next reading arrived. Changing pages sets [`App::tip`] instead.
fn fill_rail(app: &Rc<App>) {
    while let Some(child) = app.rail.first_child() {
        app.rail.remove(&child);
    }
    app.rail.append(&status_card(app));
    if let Some(actions) = actions_card(app) {
        app.rail.append(&actions);
    }
    app.rail.append(&tip_card(app));
}

fn rail_card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 13);
    card.add_css_class("rail-card");
    card
}

fn status_card(app: &Rc<App>) -> gtk::Box {
    let system = app.system();
    let list = checks::all(&system);
    let (state, ..) = checks::verdict(&list);
    let (good, total) = checks::score(&list);

    let card = rail_card();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let title = gtk::Label::new(Some("System status"));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.add_css_class("rail-title");
    head.append(&title);
    let pip = glyph(if state == State::Problem {
        "dialog-warning-symbolic"
    } else {
        "emblem-ok-symbolic"
    });
    pip.add_css_class("status-pip");
    pip.add_css_class(if state == State::Problem {
        "warn"
    } else {
        "good"
    });
    head.append(&pip);
    card.append(&head);

    let summary = gtk::Label::new(Some(&match state {
        State::Problem => format!("{} of {total} checks clear", good),
        _ => "All systems ready".to_string(),
    }));
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.add_css_class("rail-lede");
    card.append(&summary);

    let rule = gtk::Separator::new(gtk::Orientation::Horizontal);
    card.append(&rule);

    // Three rings. The GPU is the one being played on, not card zero.
    let gauges = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    gauges.set_homogeneous(true);
    let gaming_index = system
        .gpus
        .iter()
        .position(|g| gpu::gaming_gpu(&system.gpus).is_some_and(|p| p.address == g.address));
    gauges.append(&gauge(app, "CPU", move |r| {
        r.cpu.utilization.map(|v| v / 100.0)
    }));
    gauges.append(&gauge(app, "GPU", move |r| {
        gaming_index
            .and_then(|i| r.gpus.get(i))
            .and_then(|g| g.utilization)
            .map(|v| v as f64 / 100.0)
    }));
    gauges.append(&gauge(app, "RAM", |r| {
        r.memory.is_known().then(|| r.memory.fraction())
    }));
    card.append(&gauges);

    card.append(&meter(app, "VRAM", move |r| {
        let reading = gaming_index.and_then(|i| r.gpus.get(i))?;
        let (used, total) = (reading.memory_used_mb?, reading.memory_total_mb?);
        (total > 0).then(|| {
            (
                used as f64 / total as f64,
                format!(
                    "{:.1} / {:.1} GB",
                    used as f64 / 1024.0,
                    total as f64 / 1024.0
                ),
            )
        })
    }));
    card.append(&meter(app, "Storage", |r| {
        r.storage.is_known().then(|| {
            (
                r.storage.fraction(),
                format!(
                    "{} / {}",
                    tune::human_bytes(r.storage.used),
                    tune::human_bytes(r.storage.total)
                ),
            )
        })
    }));
    card
}

/// A ring with its value in the middle and its name underneath. `value`
/// returns 0..1, or `None` when there is no reading — and then the ring
/// stays empty and the number is a dash, rather than drawing a confident
/// zero.
fn gauge(
    app: &Rc<App>,
    caption: &str,
    value: impl Fn(&Readings) -> Option<f64> + Clone + 'static,
) -> gtk::Box {
    const SIZE: i32 = 58;
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_halign(gtk::Align::Center);

    let fraction = Rc::new(Cell::new(0.0f64));
    let area = gtk::DrawingArea::new();
    area.set_content_width(SIZE);
    area.set_content_height(SIZE);
    let accent = app.accent;
    let drawn = fraction.clone();
    area.set_draw_func(move |_, cr, width, height| {
        let size = width.min(height) as f64;
        let value = drawn.get();
        art::ring(cr, size, value, art::load_colour(value, accent));
    });

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    let readout = gtk::Label::new(Some("—"));
    readout.add_css_class("ring-value");
    overlay.add_overlay(&readout);
    column.append(&overlay);

    let name = gtk::Label::new(Some(caption));
    name.add_css_class("ring-caption");
    column.append(&name);

    let take = value.clone();
    app.live.bind(move |r| {
        let current = take(r);
        fraction.set(current.unwrap_or(0.0));
        readout.set_text(&match current {
            Some(v) => format!("{:.0}%", v * 100.0),
            None => "—".into(),
        });
        area.queue_draw();
    });
    column
}

/// A named bar with its figures on the right.
fn meter(
    app: &Rc<App>,
    caption: &str,
    value: impl Fn(&Readings) -> Option<(f64, String)> + 'static,
) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 5);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(caption));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.add_css_class("meter-name");
    head.append(&name);
    let figures = gtk::Label::new(Some("—"));
    figures.set_xalign(1.0);
    figures.add_css_class("meter-value");
    head.append(&figures);
    column.append(&head);

    let bar = gtk::ProgressBar::new();
    column.append(&bar);

    let bar_handle = bar.clone();
    app.live.bind(move |r| match value(r) {
        Some((fraction, text)) => {
            bar_handle.set_fraction(fraction.clamp(0.0, 1.0));
            figures.set_text(&text);
            column_visible(&bar_handle, true);
        }
        None => {
            bar_handle.set_fraction(0.0);
            figures.set_text("—");
            column_visible(&bar_handle, true);
        }
    });
    column
}

fn column_visible(widget: &impl IsA<gtk::Widget>, visible: bool) {
    widget.set_visible(visible);
}

/// The launchers and folders worth one click. Only what is installed
/// appears; a list of things that are not there is not a shortcut.
fn actions_card(app: &Rc<App>) -> Option<gtk::Box> {
    let mut rows: Vec<(String, &'static str, RailAction)> = Vec::new();
    for (command, label, icon) in games::launchers() {
        rows.push((
            label.to_string(),
            icon,
            RailAction::Run(command.to_string()),
        ));
    }
    if capture::recordings_dir().is_dir() {
        rows.push((
            "Open recordings".into(),
            "folder-videos-symbolic",
            RailAction::Open(capture::recordings_dir()),
        ));
    }
    if drivers::which("raven-settings").is_some() {
        rows.push((
            "Raven Settings".into(),
            "preferences-system-symbolic",
            RailAction::Run("raven-settings".into()),
        ));
    }
    if ["raven-terminal", "foot", "alacritty", "kitty", "xterm"]
        .iter()
        .any(|t| drivers::which(t).is_some())
    {
        rows.push((
            "Open a terminal".into(),
            "utilities-terminal-symbolic",
            RailAction::Terminal,
        ));
    }
    if rows.is_empty() {
        return None;
    }

    let card = rail_card();
    let title = gtk::Label::new(Some("Quick actions"));
    title.set_xalign(0.0);
    title.add_css_class("rail-title");
    card.append(&title);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    for (label, icon, action) in rows {
        list.append(&action_row(app, &label, icon, action));
    }
    card.append(&list);
    Some(card)
}

enum RailAction {
    /// Start a program by name.
    Run(String),
    /// Hand a path to the file manager.
    Open(std::path::PathBuf),
    Terminal,
}

fn action_row(app: &Rc<App>, label: &str, icon: &str, action: RailAction) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("action-row");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let image = glyph(icon);
    content.append(&image);
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.add_css_class("action-label");
    content.append(&text);
    button.set_child(Some(&content));

    let name = label.to_string();
    button.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| {
            let outcome = match &action {
                RailAction::Run(command) => run_detached(command, &[]),
                RailAction::Open(path) => capture::open_in_file_manager(path),
                RailAction::Terminal => open_terminal(),
            };
            match outcome {
                Ok(()) => app.toast(&name),
                Err(error) => app.toast(&error),
            }
        }
    ));
    button
}

fn run_detached(command: &str, args: &[&str]) -> Result<(), String> {
    let binary = drivers::which(command).ok_or(format!("{command} is not installed"))?;
    std::process::Command::new(binary)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start {command}: {e}"))
}

fn open_terminal() -> Result<(), String> {
    ["raven-terminal", "foot", "alacritty", "kitty", "xterm"]
        .iter()
        .find(|t| drivers::which(t).is_some())
        .ok_or("No terminal emulator is installed".to_string())
        .and_then(|terminal| run_detached(terminal, &[]))
}

fn tip_card(app: &Rc<App>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 9);
    card.add_css_class("tip-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    let bulb = glyph("dialog-information-symbolic");
    bulb.add_css_class("tip-icon");
    head.append(&bulb);
    let title = gtk::Label::new(Some("Raven tip"));
    title.set_xalign(0.0);
    title.add_css_class("tip-title");
    head.append(&title);
    card.append(&head);
    app.tip.set_xalign(0.0);
    app.tip.set_wrap(true);
    app.tip.add_css_class("tip-body");
    card.append(&app.tip);
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("tip-rule");
    rule.set_halign(gtk::Align::Start);
    card.append(&rule);
    card
}

// ---- Overview ------------------------------------------------------------

fn overview_page(app: &Rc<App>) -> gtk::Widget {
    let page = page_box();
    page.append(&hero(app));
    page.append(&quick_cards(app));
    page.append(&readiness_card(app));
    page.append(&shelf(app));
    page_scroll(&page)
}

// ---- hero ----------------------------------------------------------------

/// The banner: a painted night scene with three slides over it.
///
/// A carousel rather than one fixed panel because the three things worth
/// saying at the top of this window — is it ready, what is it playing on,
/// and how to keep a clip of it — are equally important and none of them
/// deserves to be the one that gets cut.
fn hero(app: &Rc<App>) -> gtk::Widget {
    let frame = gtk::Overlay::new();
    frame.add_css_class("hero-frame");

    let scene = gtk::DrawingArea::new();
    // The banner's height, fixed, and the reason the slides cap their
    // lines.
    //
    // A page inside a scroller is allocated its *minimum* height, not its
    // natural one — the scroller's job is to be smaller than its content.
    // So a banner whose minimum is less than what its text needs renders
    // short and loses the bottom of itself, whatever its natural size
    // says. Rather than chase that with a layout callback, the painting
    // states a height and the words are capped to fit inside it.
    scene.set_content_height(HERO_HEIGHT);
    let accent = app.accent;
    scene.set_draw_func(move |_, cr, width, height| {
        art::hero(cr, width as f64, height as f64, accent);
    });
    frame.set_child(Some(&scene));

    // A stack with its own dots rather than AdwCarousel.
    //
    // The carousel lays its pages out side by side at their natural width
    // and scrolls between them, so in a banner wider than one page the
    // next page sits visibly inside the frame — two headlines at once.
    // A stack shows exactly one child at exactly the allocated size, which
    // is what a banner wants; the swipe is not worth the other thing.
    let slides = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .hhomogeneous(true)
        .vhomogeneous(true)
        .build();
    slides.add_named(&hero_readiness(app), Some("readiness"));
    slides.add_named(&hero_card(app), Some("card"));
    slides.add_named(&hero_capture(app), Some("capture"));

    let dots = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    dots.add_css_class("hero-dots");
    dots.set_halign(gtk::Align::Center);
    dots.set_valign(gtk::Align::End);
    let names = ["readiness", "card", "capture"];
    let buttons: Vec<gtk::Button> = names
        .iter()
        .map(|name| {
            let dot = gtk::Button::new();
            dot.add_css_class("hero-dot");
            dot.set_tooltip_text(Some(match *name {
                "readiness" => "Readiness",
                "card" => "The card you play on",
                _ => "Capture",
            }));
            dots.append(&dot);
            dot
        })
        .collect();
    for (index, dot) in buttons.iter().enumerate() {
        dot.connect_clicked(glib::clone!(
            #[weak]
            slides,
            move |_| slides.set_visible_child_name(names[index])
        ));
    }
    // One place decides which dot is lit, so the dots cannot disagree with
    // the stack however the slide was changed.
    let light = {
        let buttons = buttons.clone();
        move |slides: &gtk::Stack| {
            let showing = slides.visible_child_name().unwrap_or_default();
            for (index, dot) in buttons.iter().enumerate() {
                if names[index] == showing {
                    dot.add_css_class("on");
                } else {
                    dot.remove_css_class("on");
                }
            }
        }
    };
    light(&slides);
    slides.connect_visible_child_name_notify(move |slides| light(slides));

    let stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
    stack.append(&slides);
    stack.append(&dots);
    frame.add_overlay(&stack);
    frame.set_measure_overlay(&stack, true);
    frame.upcast()
}

/// The banner's smallest height, and the lines a slide may use.
///
/// A page inside a scroller is allocated its *minimum* height, not its
/// natural one — the scroller's whole job is to be smaller than its
/// content. So the banner's minimum has to be the truth about what it
/// needs, which is what capping the lines guarantees: with a bound on how
/// far the words can wrap, the slide's minimum is its real height and the
/// painting simply follows it. The constant is only a floor, so a short
/// headline still gets a landscape rather than a stripe.
const HERO_HEIGHT: i32 = 258;
const HERO_TITLE_LINES: i32 = 2;
const HERO_LEDE_LINES: i32 = 3;

/// The shell every slide shares: words on the left, a column of three
/// lines on the right.
fn hero_slide(
    eyebrow: &str,
    title: &str,
    lede: &str,
    action: Option<gtk::Widget>,
    features: Vec<(&str, String, String)>,
) -> gtk::Box {
    let slide = gtk::Box::new(gtk::Orientation::Horizontal, 26);
    slide.add_css_class("hero-slide");

    let words = gtk::Box::new(gtk::Orientation::Vertical, 8);
    words.set_valign(gtk::Align::Center);
    words.set_hexpand(true);
    let eyebrow_label = gtk::Label::new(Some(&eyebrow.to_uppercase()));
    eyebrow_label.set_xalign(0.0);
    eyebrow_label.add_css_class("hero-eyebrow");
    words.append(&eyebrow_label);
    // Capped rather than free-flowing: see HERO_HEIGHT. An ellipsis on a
    // headline is a worse look than a headline that wraps, and a better
    // one than a button that has fallen off the bottom of the banner.
    let title_label = gtk::Label::new(Some(title));
    title_label.set_xalign(0.0);
    // Capped rather than stretched: a label with no halign takes the
    // whole column, and a 34px headline running the full width of a wide
    // window is a banner nobody reads the end of.
    title_label.set_halign(gtk::Align::Start);
    title_label.set_max_width_chars(26);
    title_label.set_wrap(true);
    title_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title_label.set_lines(HERO_TITLE_LINES);
    title_label.add_css_class("hero-title");
    words.append(&title_label);
    let lede_label = gtk::Label::new(Some(lede));
    lede_label.set_xalign(0.0);
    lede_label.set_halign(gtk::Align::Start);
    lede_label.set_wrap(true);
    lede_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    lede_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    lede_label.set_lines(HERO_LEDE_LINES);
    lede_label.set_max_width_chars(52);
    lede_label.add_css_class("hero-lede");
    words.append(&lede_label);
    if let Some(action) = action {
        action.set_halign(gtk::Align::Start);
        action.set_margin_top(10);
        words.append(&action);
    }
    slide.append(&words);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 15);
    column.set_valign(gtk::Align::Center);
    column.set_size_request(280, -1);
    for (icon, title, detail) in features {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 13);
        let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tile.add_css_class("hero-feature-icon");
        tile.set_valign(gtk::Align::Center);
        tile.set_hexpand(false);
        let glyph = glyph(icon);
        glyph.set_halign(gtk::Align::Center);
        glyph.set_hexpand(true);
        tile.append(&glyph);
        row.append(&tile);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_valign(gtk::Align::Center);
        let heading = gtk::Label::new(Some(&title));
        heading.set_xalign(0.0);
        heading.set_ellipsize(gtk::pango::EllipsizeMode::End);
        heading.add_css_class("hero-feature-title");
        text.append(&heading);
        let sub = gtk::Label::new(Some(&detail));
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        sub.add_css_class("hero-feature-sub");
        text.append(&sub);
        row.append(&text);
        column.append(&row);
    }
    slide.append(&column);
    slide
}

fn cta(label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("cta");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    let text = gtk::Label::new(Some(label));
    content.append(&text);
    content.append(&glyph("go-next-symbolic"));
    button.set_child(Some(&content));
    button
}

fn hero_readiness(app: &Rc<App>) -> gtk::Box {
    let system = app.system();
    let list = checks::all(&system);
    let (state, headline, blurb) = checks::verdict(&list);
    let fixes = checks::combined_fixes(&list);

    // The button installs packages and asks for a password, so it says
    // what it is about to do before it is pressed rather than after.
    let action: Option<gtk::Widget> = if fixes.is_empty() {
        None
    } else {
        let button = cta("Fix what is missing");
        button.set_halign(gtk::Align::Start);
        button.connect_clicked(glib::clone!(
            #[strong]
            app,
            #[strong]
            fixes,
            move |_| run_fixes(&app, fixes.clone())
        ));
        Some(button.upcast())
    };

    // The three lines on the right are the checks that are not clear —
    // the actual work outstanding — and when there is none, the three
    // things that being ready means.
    let outstanding: Vec<&checks::Check> = list
        .iter()
        .filter(|c| c.state != State::Good)
        .take(3)
        .collect();
    let features = if outstanding.is_empty() {
        vec![
            (
                "emblem-ok-symbolic",
                "Drivers in place".to_string(),
                "Kernel and libraries both".to_string(),
            ),
            (
                "emblem-ok-symbolic",
                "Built for every kernel".to_string(),
                "A kernel update will not break it".to_string(),
            ),
            (
                "emblem-ok-symbolic",
                "Settings applied".to_string(),
                "Large games have the room they need".to_string(),
            ),
        ]
    } else {
        outstanding
            .iter()
            .map(|check| {
                (
                    check.state.icon(),
                    check.title.clone(),
                    check
                        .detail
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(60)
                        .collect::<String>(),
                )
            })
            .collect()
    };

    // What the button is about to do, said in the paragraph above it
    // rather than in a label of its own. A button that installs software
    // and asks for a password should say so while there is still the
    // option of not pressing it, and one more line under the button is
    // the line that falls off the bottom of the banner.
    let lede = match fix_summary(&fixes) {
        summary if summary.is_empty() => blurb.to_string(),
        summary => format!("{blurb} Fixing it means {summary}."),
    };
    hero_slide(&state.to_string(), headline, &lede, action, features)
}

/// "4 packages, driver modules for 1 kernel, and system settings."
///
/// Written out under the button rather than left to a dialog, because a
/// button that installs software and asks for a password should say what
/// it is going to do while there is still the option of not pressing it.
fn fix_summary(fixes: &[Fix]) -> String {
    let mut parts = Vec::new();
    for fix in fixes {
        match fix {
            Fix::Install(names) => parts.push(format!(
                "{} package{}",
                names.len(),
                if names.len() == 1 { "" } else { "s" }
            )),
            Fix::BuildModules(kernels) => parts.push(format!(
                "driver modules for {} kernel{}",
                kernels.len(),
                if kernels.len() == 1 { "" } else { "s" }
            )),
            Fix::ApplyTweaks => parts.push("system settings".into()),
            Fix::Manual(_) => {}
        }
    }
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => format!(
            "{}, and {}",
            parts[..parts.len() - 1].join(", "),
            parts[parts.len() - 1]
        ),
    }
}

fn hero_card(app: &Rc<App>) -> gtk::Box {
    let system = app.system();
    let Some(card) = gpu::gaming_gpu(&system.gpus) else {
        return hero_slide(
            "Graphics",
            "No graphics card found",
            "Nothing on the PCI bus identifies itself as a display controller. Inside a virtual machine that is normal.",
            None,
            Vec::new(),
        );
    };
    let index = system
        .gpus
        .iter()
        .position(|g| g.address == card.address)
        .unwrap_or(0);

    let button = cta("Graphics drivers");
    button.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| jump_to(&app, "graphics")
    ));

    let lede = if gpu::is_hybrid(&system.gpus) {
        "The faster of the two cards in this machine. A game only lands on it when it is told to — the Library page has the line that tells it."
    } else {
        "The card every game in this machine renders on."
    };
    let slide = hero_slide(
        "Playing on",
        &card.title(),
        lede,
        Some(button.upcast()),
        vec![
            (
                "utilities-system-monitor-symbolic",
                "Load".into(),
                "—".into(),
            ),
            ("temperature-symbolic", "Temperature".into(), "—".into()),
            ("battery-full-charging-symbolic", "Power".into(), "—".into()),
        ],
    );
    // The three lines are live, so they are found again and bound rather
    // than built twice.
    bind_hero_features(app, &slide, index);
    slide
}

/// Binds the three feature sub-labels of a slide to the live readings.
///
/// The slide is built by the shared helper, so the labels are reached by
/// walking it rather than by returning six handles from a function whose
/// other callers do not want them.
fn bind_hero_features(app: &Rc<App>, slide: &gtk::Box, gpu_index: usize) {
    let Some(column) = slide.last_child() else {
        return;
    };
    let mut row = column.first_child();
    let mut which = 0;
    while let Some(current) = row {
        if let Some(text) = current.last_child()
            && let Some(sub) = text.last_child()
            && let Some(label) = sub.downcast_ref::<gtk::Label>()
        {
            let label = label.clone();
            match which {
                0 => app.live.bind(move |r| {
                    label.set_text(&telemetry::percent(
                        r.gpus.get(gpu_index).and_then(|g| g.utilization),
                    ))
                }),
                1 => app.live.bind(move |r| {
                    label.set_text(&telemetry::degrees(
                        r.gpus.get(gpu_index).and_then(|g| g.temperature_c),
                    ))
                }),
                _ => app.live.bind(move |r| {
                    label.set_text(&telemetry::watts(
                        r.gpus.get(gpu_index).and_then(|g| g.power_w),
                    ))
                }),
            }
        }
        which += 1;
        row = current.next_sibling();
    }
}

fn hero_capture(app: &Rc<App>) -> gtk::Box {
    let recordings = capture::recordings();
    let button = cta("Open Capture");
    button.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| jump_to(&app, "capture")
    ));
    let kept = match recordings.len() {
        0 => "Nothing recorded yet".to_string(),
        1 => "1 recording kept".to_string(),
        n => format!("{n} recordings kept"),
    };
    hero_slide(
        "Capture",
        "Keep the run that went right",
        "The compositor records the screen itself, inside the render loop, so a game working hard cannot make it drop frames.",
        Some(button.upcast()),
        vec![
            (
                "input-keyboard-symbolic",
                "Super + Print".into(),
                "Starts and stops a recording".into(),
            ),
            (
                "folder-videos-symbolic",
                kept,
                "In your Videos folder".into(),
            ),
            (
                "video-x-generic-symbolic",
                if capture::exporter_available() {
                    "Export to MP4".into()
                } else {
                    "raven-export missing".into()
                },
                "Playable anywhere".into(),
            ),
        ],
    )
}

// ---- quick cards ---------------------------------------------------------

/// The four controls worth reaching without changing page.
fn quick_cards(app: &Rc<App>) -> gtk::FlowBox {
    let row = card_row(4);
    row.append(&profile_quick_card(app));
    row.append(&clocks_quick_card(app));
    row.append(&display_quick_card(app));
    row.append(&tuning_quick_card(app));
    row
}

/// The shell of a quick card: a tinted glyph, a name, and room for a
/// control and a footnote.
fn quick_card(icon: &str, name: &str, lit: bool) -> (gtk::Box, gtk::Box) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("quick-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 11);
    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("quick-tile");
    if lit {
        tile.add_css_class("is-on");
    }
    tile.set_valign(gtk::Align::Center);
    tile.set_hexpand(false);
    let glyph = glyph(icon);
    glyph.set_halign(gtk::Align::Center);
    glyph.set_hexpand(true);
    tile.append(&glyph);
    head.append(&tile);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    // A homogeneous row is as wide as its widest card's *natural* width,
    // so one card with a long unwrapped line makes all four too wide to
    // sit on one line. Capping the natural width keeps the row a row.
    label.set_max_width_chars(20);
    label.add_css_class("quick-eyebrow");
    head.append(&label);
    card.append(&head);
    (card.clone(), card)
}

fn quick_foot(card: &gtk::Box, text: &str) {
    let foot = gtk::Label::new(Some(text));
    foot.set_xalign(0.0);
    foot.set_wrap(true);
    foot.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    foot.set_max_width_chars(30);
    foot.add_css_class("quick-foot");
    card.append(&foot);
}

fn profile_quick_card(app: &Rc<App>) -> gtk::Box {
    let system = app.system();
    let active = tune::active_preset();
    let (card, body) = quick_card(
        "power-profile-performance-symbolic",
        "Performance profile",
        active == Some(tune::Preset::Performance),
    );

    let names: Vec<&str> = tune::PRESETS.iter().map(|p| p.title()).collect();
    let choices = gtk::StringList::new(&names);
    let dropdown = gtk::DropDown::new(Some(choices), gtk::Expression::NONE);
    dropdown.set_halign(gtk::Align::Start);
    match active.and_then(|a| tune::PRESETS.iter().position(|p| *p == a)) {
        Some(index) => dropdown.set_selected(index as u32),
        None => dropdown.set_sensitive(false),
    }
    let cards: Vec<String> = system
        .gpus
        .iter()
        .filter(|g| tune::gpu_mode_path(g).is_some())
        .filter_map(|g| g.card.clone())
        .collect();
    dropdown.connect_selected_notify(glib::clone!(
        #[strong]
        app,
        move |dropdown| {
            let Some(preset) = tune::PRESETS.get(dropdown.selected() as usize).copied() else {
                return;
            };
            if active == Some(preset) {
                return;
            }
            apply_preset(&app, preset, &cards);
        }
    ));
    body.append(&dropdown);
    quick_foot(
        &body,
        match active {
            Some(preset) => preset.description(),
            None => "No power-profile service is running, so this cannot be set from here.",
        },
    );
    card
}

fn clocks_quick_card(app: &Rc<App>) -> gtk::Box {
    let system = app.system();
    let tunable = system
        .gpus
        .iter()
        .find(|g| tune::gpu_mode_path(g).is_some())
        .cloned();
    let current = tunable.as_ref().and_then(tune::gpu_mode);
    let (card, body) = quick_card(
        "applications-graphics-symbolic",
        "Graphics clocks",
        current == Some(tune::GpuMode::High),
    );

    let Some(gpu) = tunable else {
        // No card here exposes a clock policy — NVIDIA does not — so the
        // card reports the driver instead of showing a control that could
        // not do anything.
        let value = gtk::Label::new(Some(
            &gpu::gaming_gpu(&system.gpus)
                .map(|g| g.driver.label().to_string())
                .unwrap_or_else(|| "No card".into()),
        ));
        value.set_xalign(0.0);
        value.add_css_class("quick-value");
        body.append(&value);
        quick_foot(
            &body,
            "This driver has no clock policy to set. It manages clocks on its own.",
        );
        return card;
    };

    let modes = [tune::GpuMode::Low, tune::GpuMode::Auto, tune::GpuMode::High];
    let names: Vec<&str> = modes.iter().map(|m| m.title()).collect();
    let dropdown = gtk::DropDown::new(Some(gtk::StringList::new(&names)), gtk::Expression::NONE);
    dropdown.set_halign(gtk::Align::Start);
    if let Some(index) = current.and_then(|c| modes.iter().position(|m| *m == c)) {
        dropdown.set_selected(index as u32);
    }
    let card_name = gpu.card.clone();
    dropdown.connect_selected_notify(glib::clone!(
        #[strong]
        app,
        move |dropdown| {
            let Some(mode) = modes.get(dropdown.selected() as usize).copied() else {
                return;
            };
            if current == Some(mode) {
                return;
            }
            let Some(name) = card_name.clone() else {
                return;
            };
            run_root_actions(
                &app,
                &format!("{} clocks", mode.title()),
                vec![tune::Action::GpuMode { card: name, mode }],
            );
        }
    ));
    body.append(&dropdown);
    quick_foot(&body, &format!("On {}", gpu.title()));
    card
}

fn display_quick_card(app: &Rc<App>) -> gtk::Box {
    let mode = current_display_mode();
    let (card, body) = quick_card("video-display-symbolic", "Display", false);
    let value = gtk::Label::new(Some(mode.as_deref().unwrap_or("Unknown")));
    value.set_xalign(0.0);
    value.set_wrap(true);
    value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    value.set_max_width_chars(24);
    value.add_css_class("quick-value");
    body.append(&value);

    if drivers::which("raven-settings").is_some() {
        let open = gtk::Button::with_label("Display settings");
        open.add_css_class("link-button");
        open.set_halign(gtk::Align::Start);
        open.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| match run_detached("raven-settings", &[]) {
                Ok(()) => app.toast("Opening Raven Settings"),
                Err(error) => app.toast(&error),
            }
        ));
        body.append(&open);
    } else {
        quick_foot(
            &body,
            "Resolution and refresh rate are Raven Settings' to change.",
        );
    }
    card
}

/// The mode the screen is running, from GDK.
///
/// Not from `/sys/class/drm`: the `modes` file lists the resolutions a
/// connector supports and says nothing about which one is in use or at
/// what refresh rate. This process is already a Wayland client with the
/// compositor's own answer to that question.
fn current_display_mode() -> Option<String> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    // The monitor this window is on, falling back to the first — on a
    // single-screen machine they are the same, and on a desk with two the
    // one being looked at is the right answer.
    let monitor = (0..monitors.n_items())
        .filter_map(|i| monitors.item(i)?.downcast::<gdk::Monitor>().ok())
        .max_by_key(|m| {
            let area = m.geometry();
            (area.width() as i64) * (area.height() as i64)
        })?;
    let area = monitor.geometry();
    let scale = monitor.scale_factor().max(1);
    let (width, height) = (area.width() * scale, area.height() * scale);
    // GDK reports the refresh rate in millihertz, and zero when the
    // compositor did not say.
    Some(match monitor.refresh_rate() {
        0 => format!("{width} × {height}"),
        milli => format!(
            "{width} × {height} · {} Hz",
            (milli as f64 / 1000.0).round()
        ),
    })
}

fn tuning_quick_card(app: &Rc<App>) -> gtk::Box {
    let needed = tune::tweaks_needed();
    let applied = needed.is_empty();
    let (card, body) = quick_card("preferences-system-symbolic", "Game tuning", applied);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let value = gtk::Label::new(Some(if applied { "Applied" } else { "Not applied" }));
    value.set_xalign(0.0);
    value.set_hexpand(true);
    value.add_css_class(if applied { "quick-value" } else { "quick-foot" });
    row.append(&value);
    let toggle = gtk::Switch::new();
    toggle.set_active(applied);
    toggle.set_valign(gtk::Align::Center);
    toggle.connect_state_set(glib::clone!(
        #[strong]
        app,
        move |toggle, wanted| {
            if wanted == applied {
                return glib::Propagation::Proceed;
            }
            // The switch is put back where it was; the page is rebuilt
            // from the system once the change has actually happened, so
            // it never shows a state that was only asked for.
            toggle.set_active(applied);
            if wanted {
                run_fixes(&app, vec![Fix::ApplyTweaks]);
            } else {
                confirm_revert(&app);
            }
            glib::Propagation::Stop
        }
    ));
    row.append(&toggle);
    body.append(&row);
    quick_foot(
        &body,
        if applied {
            "Kernel settings large games depend on are in place."
        } else {
            "Four kernel settings games need. Off, big games can fail to start."
        },
    );
    card
}

fn apply_preset(app: &Rc<App>, preset: tune::Preset, cards: &[String]) {
    if let Err(error) = tune::set_power_profile(preset.power_profile()) {
        app.toast(&error);
        return;
    }
    let actions: Vec<tune::Action> = cards
        .iter()
        .map(|card| tune::Action::GpuMode {
            card: card.clone(),
            mode: preset.gpu_mode(),
        })
        .collect();
    if actions.is_empty() {
        app.toast(&format!("{} mode", preset.title()));
        app.refresh();
    } else {
        run_root_actions(app, &format!("{} mode", preset.title()), actions);
    }
}

// ---- readiness -----------------------------------------------------------

fn readiness_card(app: &Rc<App>) -> gtk::Box {
    let system = app.system();
    let list = checks::all(&system);
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    holder.add_css_class("shelf");

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    head.set_margin_bottom(4);
    let title = gtk::Label::new(Some("Everything that matters"));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.add_css_class("shelf-title");
    head.append(&title);
    let (good, total) = checks::score(&list);
    let count = gtk::Label::new(Some(&format!("{good} of {total} clear")));
    count.add_css_class("badge");
    count.add_css_class(if good == total {
        "installed"
    } else {
        "warning"
    });
    count.set_valign(gtk::Align::Center);
    head.append(&count);
    holder.append(&head);

    for check in &list {
        let trailing = check.fix.as_ref().and_then(|fix| fix_button(app, fix));
        let detail = match &check.fix {
            Some(Fix::Manual(advice)) => format!("{}\n\n{advice}", check.detail),
            _ => check.detail.clone(),
        };
        holder.append(&check_row(
            check.state,
            &check.title,
            &detail,
            trailing.as_ref(),
        ));
    }
    holder
}

// ---- the shelf of games --------------------------------------------------

fn shelf(app: &Rc<App>) -> gtk::Box {
    let system = app.system();
    let library = games::discover();
    let installed = |name: &str| system.has(name);
    let recipe = games::launch_recipe(&system.gpus, &installed);

    let holder = gtk::Box::new(gtk::Orientation::Vertical, 14);
    holder.add_css_class("shelf");

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let title = gtk::Label::new(Some("Installed games"));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.add_css_class("shelf-title");
    head.append(&title);
    let all = gtk::Button::new();
    all.add_css_class("link-button");
    let all_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    all_content.append(&gtk::Label::new(Some("View all")));
    all_content.append(&glyph("go-next-symbolic"));
    all.set_child(Some(&all_content));
    all.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| jump_to(&app, "games")
    ));
    head.append(&all);
    holder.append(&head);

    if library.is_empty() {
        let empty = gtk::Label::new(Some(if games::steam_installed() {
            "Steam is installed and its library is empty. Anything installed through Steam, Lutris or Heroic appears here."
        } else {
            "No game launcher is installed. Steam brings Proton with it, which is what runs Windows games on Linux."
        }));
        empty.set_xalign(0.0);
        empty.set_wrap(true);
        empty.add_css_class("dim-label");
        holder.append(&empty);
        return holder;
    }

    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    for game in library.iter().take(12) {
        strip.append(&game_tile(app, game, &recipe));
    }
    let scroller = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&strip)
        .build();
    scroller.set_size_request(-1, 244);
    holder.append(&scroller);
    holder
}

const TILE_WIDTH: i32 = 158;
const TILE_HEIGHT: i32 = 226;

fn game_tile(app: &Rc<App>, game: &games::Game, recipe: &games::LaunchRecipe) -> gtk::Overlay {
    let tile = gtk::Overlay::new();
    tile.add_css_class("game-tile");
    tile.set_overflow(gtk::Overflow::Hidden);
    tile.set_size_request(TILE_WIDTH, TILE_HEIGHT);
    tile.set_valign(gtk::Align::Start);

    // Steam's own cover if it has one on disk, and a lettered plate if it
    // does not — a grey rectangle where a picture should be reads as a
    // loading failure rather than as "no artwork".
    match games::cover_art(game) {
        Some(path) => {
            let picture = gtk::Picture::for_filename(&path);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_can_shrink(true);
            tile.set_child(Some(&picture));
        }
        None => {
            let plate = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            plate.add_css_class("cover-fallback");
            let letter = gtk::Label::new(Some(
                &game
                    .name
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".into()),
            ));
            letter.set_hexpand(true);
            letter.set_vexpand(true);
            letter.add_css_class("cover-letter");
            plate.append(&letter);
            tile.set_child(Some(&plate));
        }
    }

    let foot = gtk::Box::new(gtk::Orientation::Vertical, 8);
    foot.add_css_class("cover-scrim");
    foot.set_valign(gtk::Align::End);
    let name = gtk::Label::new(Some(&game.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("game-name");
    foot.append(&name);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if games::launch_command(game).is_some() {
        let play = gtk::Button::from_icon_name(&icon_name("media-playback-start-symbolic"));
        play.add_css_class("play");
        play.set_tooltip_text(Some(&format!("Play {}", game.name)));
        let target = game.clone();
        play.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| match games::launch(&target) {
                Ok(()) => app.toast(&format!("Starting {}", target.name)),
                Err(error) => app.toast(&error),
            }
        ));
        buttons.append(&play);
    } else {
        let note = gtk::Label::new(Some(game.source.name()));
        note.set_valign(gtk::Align::Center);
        note.add_css_class("game-note");
        buttons.append(&note);
    }

    let menu = gtk::MenuButton::new();
    menu.set_icon_name(&icon_name("view-more-symbolic"));
    menu.add_css_class("tile-menu");
    menu.set_halign(gtk::Align::End);
    menu.set_hexpand(true);
    menu.set_valign(gtk::Align::Center);
    menu.set_popover(Some(&tile_menu(app, game, recipe)));
    buttons.append(&menu);
    foot.append(&buttons);
    tile.add_overlay(&foot);
    tile
}

fn tile_menu(app: &Rc<App>, game: &games::Game, recipe: &games::LaunchRecipe) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);

    let options = recipe.steam_launch_options();
    if game.source == games::Source::Steam && !recipe.is_empty() {
        let copy = gtk::Button::new();
        copy.add_css_class("action-row");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.append(&glyph("edit-copy-symbolic"));
        let label = gtk::Label::new(Some("Copy launch options"));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        content.append(&label);
        copy.set_child(Some(&content));
        copy.connect_clicked(glib::clone!(
            #[strong]
            app,
            #[weak]
            popover,
            move |button| {
                button.clipboard().set_text(&options);
                popover.popdown();
                app.toast("Copied — paste into the game's Launch Options in Steam");
            }
        ));
        list.append(&copy);
    }

    if let Some(dir) = game.install_dir.clone().filter(|d| d.is_dir()) {
        let open = gtk::Button::new();
        open.add_css_class("action-row");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.append(&glyph("folder-open-symbolic"));
        let label = gtk::Label::new(Some("Open install folder"));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        content.append(&label);
        open.set_child(Some(&content));
        open.connect_clicked(glib::clone!(
            #[strong]
            app,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                if let Err(error) = capture::open_in_file_manager(&dir) {
                    app.toast(&error);
                }
            }
        ));
        list.append(&open);
    }

    if list.first_child().is_none() {
        let nothing = gtk::Label::new(Some("Nothing to do for this one"));
        nothing.set_margin_top(8);
        nothing.set_margin_bottom(8);
        nothing.set_margin_start(10);
        nothing.set_margin_end(10);
        nothing.add_css_class("dim-label");
        list.append(&nothing);
    }
    popover.set_child(Some(&list));
    popover
}
// ---- Graphics drivers ----------------------------------------------------

fn graphics_page(app: &Rc<App>) -> gtk::Widget {
    let system = app.system();
    let page = page_box();

    if system.gpus.is_empty() {
        return empty_state(
            "video-display-symbolic",
            "No graphics card found",
            "Nothing on the PCI bus identifies itself as a display controller. Inside a virtual machine that is normal.",
        );
    }

    for (index, gpu) in system.gpus.iter().enumerate() {
        page.append(&gpu_card(app, &system, gpu, index));
    }

    // ---- which card a game lands on ----
    if gpu::is_hybrid(&system.gpus) {
        page.append(&section_title(
            "Two graphics cards",
            "This machine has a discrete card and an integrated one. Which of them a game uses is not automatic.",
        ));
        let explain = card();
        let default_card = games::default_render_gpu(&system.gpus)
            .map(|g| g.title())
            .unwrap_or_else(|| "unknown".into());
        let fast_card = gpu::gaming_gpu(&system.gpus)
            .map(|g| g.title())
            .unwrap_or_else(|| "unknown".into());
        let same = default_card == fast_card;
        explain.append(&check_row(
            if same { State::Good } else { State::Advisory },
            "Where a game goes by default",
            &if same {
                format!("A game started with no special options runs on {default_card}, which is the faster card. Nothing to do.")
            } else {
                format!(
                    "A game started with no special options runs on {default_card} — the card the desktop is already on. {fast_card} is the faster one, and a game only uses it when it is told to. This is the single most common reason a game is inexplicably slow on a laptop.\n\nThe Games page has the line to paste into Steam's launch options."
                )
            },
            None,
        ));
        page.append(&explain);
    }

    // ---- DKMS ----
    page.append(&dkms_section(app, &system));

    // ---- Vulkan ----
    page.append(&section_title(
        "Vulkan",
        "The graphics interface almost every modern game uses, and the one Proton translates Windows games into.",
    ));
    let vulkan = card();
    let icds = gpu::vulkan_icds();
    if icds.is_empty() {
        vulkan.append(&check_row(
            State::Problem,
            "No Vulkan driver",
            "No driver has registered itself with the Vulkan loader.",
            None,
        ));
    } else {
        for icd in &icds {
            let relevant = system.gpus.iter().any(|g| g.vendor == icd.vendor);
            let (state, detail) = if !icd.library_present {
                (
                    State::Problem,
                    "The manifest is installed but the driver library it names is not on disk. This is a half-removed package.".to_string(),
                )
            } else if relevant {
                (
                    State::Good,
                    format!(
                        "{} — drives the {} graphics in this machine.",
                        icd.file, icd.vendor
                    ),
                )
            } else {
                (
                    State::Advisory,
                    format!(
                        "{} — installed, but there is no {} card here for it to drive.",
                        icd.file, icd.vendor
                    ),
                )
            };
            vulkan.append(&check_row(state, icd.vendor.name(), &detail, None));
        }
    }
    let probe = gtk::Button::with_label("Test Vulkan");
    probe.add_css_class("pill");
    probe.set_halign(gtk::Align::Start);
    probe.set_tooltip_text(Some(
        "Runs vkcube, which draws a spinning cube if Vulkan works",
    ));
    probe.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| match drivers::which("vkcube") {
            Some(vkcube) => match std::process::Command::new(vkcube).spawn() {
                Ok(_) => app.toast("Opened vkcube — a spinning cube means Vulkan works"),
                Err(error) => app.toast(&format!("Could not start vkcube: {error}")),
            },
            None => app.toast("vkcube is not installed; it comes with vulkan-tools"),
        }
    ));
    vulkan.append(&probe);
    page.append(&vulkan);

    page_scroll(&page)
}

fn gpu_card(app: &Rc<App>, system: &checks::System, card_gpu: &Gpu, index: usize) -> gtk::Box {
    let holder = card();
    holder.add_css_class("gpu-card");

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class(card_gpu.vendor.tint());
    tile.add_css_class("large");
    tile.set_valign(gtk::Align::Start);
    tile.set_hexpand(false);
    let glyph = glyph("video-display-symbolic");
    glyph.set_halign(gtk::Align::Center);
    glyph.set_hexpand(true);
    tile.append(&glyph);
    head.append(&tile);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&card_gpu.title()));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.add_css_class("gpu-name");
    text.append(&name);
    let role = if card_gpu.is_integrated() {
        "Integrated graphics"
    } else if system.gpus.len() > 1 {
        "Discrete graphics"
    } else {
        "Graphics"
    };
    let subtitle = gtk::Label::new(Some(&format!(
        "{} · {role} · {:04x}:{:04x} · module {}{}",
        card_gpu.vendor,
        card_gpu.vendor_id,
        card_gpu.device_id,
        card_gpu.module.as_deref().unwrap_or("none"),
        card_gpu
            .card
            .as_deref()
            .map(|c| format!(" · {c}"))
            .unwrap_or_default()
    )));
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.add_css_class("dim-label");
    text.append(&subtitle);
    head.append(&text);

    let badge = gtk::Label::new(Some(card_gpu.driver.label()));
    badge.add_css_class("badge");
    badge.add_css_class(if card_gpu.driver.plays_games() {
        "installed"
    } else {
        "warning"
    });
    badge.set_valign(gtk::Align::Start);
    head.append(&badge);
    holder.append(&head);
    holder.append(&gpu_readout(app, index));

    let card_missing: Vec<&drivers::Requirement> = system
        .missing
        .iter()
        .filter(|r| requirement_belongs_to(&r.package, card_gpu.vendor))
        .collect();
    let owned: Vec<drivers::Requirement> = card_missing.iter().map(|r| (*r).clone()).collect();
    let verdict = drivers::verdict(card_gpu, &owned);
    holder.append(&check_row(
        if verdict.good {
            State::Good
        } else {
            State::Problem
        },
        &verdict.headline,
        &verdict.detail,
        None,
    ));

    for requirement in &owned {
        let install = gtk::Button::with_label("Install");
        install.add_css_class("pill");
        install.add_css_class("suggested-action");
        let package = requirement.package.clone();
        install.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| run_fixes(&app, vec![Fix::Install(vec![package.clone()])])
        ));
        let widget: gtk::Widget = install.upcast();
        holder.append(&check_row(
            if requirement.essential {
                State::Problem
            } else {
                State::Advisory
            },
            &requirement.package,
            &requirement.reason,
            Some(&widget),
        ));
    }

    // The render node is the thing a game actually opens. Its absence is
    // why "the driver is installed" and "the card works" can differ.
    let node = card_gpu
        .render_node
        .as_ref()
        .map(|p| p.display().to_string());
    holder.append(&check_row(
        if node.is_some() { State::Good } else { State::Problem },
        "Render device",
        &match &node {
            Some(path) => format!("{path} — this is the device a game opens to render on."),
            None => "This card has no render device, so nothing can render on it even though it is in the machine.".into(),
        },
        None,
    ));
    holder
}

/// The five live numbers for one card.
///
/// Every one is a dash until a reading arrives, and stays a dash on a card
/// that reports nothing — an NVIDIA card with no `nvidia-smi` installed
/// reports nothing at all, and five zeroes would be a lie about a working
/// card rather than an admission that nothing was read.
fn gpu_readout(app: &Rc<App>, index: usize) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    row.add_css_class("data-row");
    /// A caption and the reading it shows.
    type Field = (&'static str, fn(&GpuReading) -> String);
    let fields: [Field; 5] = [
        ("LOAD", |g| telemetry::percent(g.utilization)),
        ("TEMPERATURE", |g| telemetry::degrees(g.temperature_c)),
        ("POWER", |g| telemetry::watts(g.power_w)),
        ("CLOCK", |g| telemetry::megahertz(g.core_mhz)),
        ("MEMORY", |g| {
            telemetry::memory(g.memory_used_mb, g.memory_total_mb)
        }),
    ];
    for (caption, read) in fields {
        let (holder, value) = stat(caption);
        app.live.label(&value, move |r| match r.gpus.get(index) {
            Some(reading) => read(reading),
            None => "—".into(),
        });
        row.append(&holder);
    }
    row
}

/// Whether a missing package belongs to a particular card, so each card's
/// section lists only its own.
fn requirement_belongs_to(package: &str, vendor: gpu::Vendor) -> bool {
    match vendor {
        gpu::Vendor::Nvidia => package.contains("nvidia"),
        gpu::Vendor::Amd => package.contains("radeon") || package.contains("mesa"),
        gpu::Vendor::Intel => package.contains("intel"),
        gpu::Vendor::Other => false,
    }
}

fn dkms_section(app: &Rc<App>, system: &checks::System) -> gtk::Box {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 14);
    holder.append(&section_title(
        "Driver modules per kernel",
        "A DKMS driver is compiled separately for every kernel. Install a new kernel, boot it, and a driver that was never built for it is simply not there.",
    ));
    let list = card();
    if !drivers::dkms_available() {
        list.append(&check_row(
            State::Good,
            "No DKMS drivers",
            "Nothing on this system builds its driver with DKMS, so a kernel update cannot leave you without one.",
            None,
        ));
        holder.append(&list);
        return holder;
    }
    if system.dkms.is_empty() {
        list.append(&check_row(
            State::Good,
            "No DKMS drivers registered",
            "dkms is installed but has no modules, so there is nothing that a kernel update could leave behind.",
            None,
        ));
        holder.append(&list);
        return holder;
    }

    let missing = drivers::kernels_missing_modules(&system.dkms, &system.kernels);
    for kernel in &system.kernels {
        let built: Vec<&drivers::DkmsModule> = system
            .dkms
            .iter()
            .filter(|m| m.kernel == kernel.release && m.is_installed())
            .collect();
        let is_missing = missing.contains(&kernel.release);
        let detail = if built.is_empty() {
            "No graphics module is built for this kernel. Booting it would come up with no driver."
                .to_string()
        } else {
            format!(
                "{} built and installed.",
                built
                    .iter()
                    .map(|m| format!("{} {}", m.name, m.version))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let title = if kernel.running {
            format!("{} (running now)", kernel.release)
        } else {
            kernel.release.clone()
        };
        let trailing: Option<gtk::Widget> = if is_missing {
            let button = gtk::Button::with_label("Build");
            button.add_css_class("pill");
            button.add_css_class("suggested-action");
            let release = kernel.release.clone();
            button.connect_clicked(glib::clone!(
                #[strong]
                app,
                move |_| run_fixes(&app, vec![Fix::BuildModules(vec![release.clone()])])
            ));
            Some(button.upcast())
        } else {
            None
        };
        list.append(&check_row(
            if is_missing {
                State::Problem
            } else {
                State::Good
            },
            &title,
            &detail,
            trailing.as_ref(),
        ));
    }
    if missing.len() > 1 {
        let all = gtk::Button::with_label(&format!("Build for all {} kernels", missing.len()));
        all.add_css_class("pill");
        all.add_css_class("suggested-action");
        all.set_halign(gtk::Align::Start);
        let kernels = missing.clone();
        all.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| run_fixes(&app, vec![Fix::BuildModules(kernels.clone())])
        ));
        list.append(&all);
    }
    holder.append(&list);
    holder
}
// ---- Performance ---------------------------------------------------------

fn performance_page(app: &Rc<App>) -> gtk::Widget {
    let system = app.system();
    let page = page_box();

    // ---- presets ----
    page.append(&section_title(
        "How hard the machine may work",
        "One choice that sets the processor's power profile and, where the card allows it, how hard the graphics clock.",
    ));
    let active = tune::active_preset();
    let row = card_row(3);
    for preset in tune::PRESETS {
        row.append(&preset_card(app, &system, preset, active == Some(preset)));
    }
    page.append(&row);
    if active.is_none() {
        let note = gtk::Label::new(Some(
            "No power-profile service is running, so the processor's profile cannot be read or changed from here. raven-powerd or power-profiles-daemon provides it.",
        ));
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.add_css_class("info-note");
        page.append(&note);
    }

    // ---- per-card clock policy ----
    let tunable: Vec<&Gpu> = system
        .gpus
        .iter()
        .filter(|g| tune::gpu_mode_path(g).is_some())
        .collect();
    if !tunable.is_empty() {
        page.append(&section_title(
            "Graphics clocks",
            "Held high, a card keeps its frame times even and runs warmer. On automatic the driver decides, which is right everywhere but a game.",
        ));
        let list = card();
        for gpu in tunable {
            list.append(&gpu_mode_row(app, gpu));
        }
        page.append(&list);
    }

    // ---- system settings ----
    page.append(&section_title(
        "System settings games depend on",
        "Kernel defaults chosen for servers. Each one below either stops a game from running or costs smoothness, and Raven Gaming changes only these four.",
    ));
    let tweaks = card();
    for tweak in tune::TWEAKS {
        let applied = tweak.is_applied();
        let badge = gtk::Label::new(Some(if applied { "Set" } else { "Default" }));
        badge.add_css_class("badge");
        badge.add_css_class(if applied { "installed" } else { "neutral" });
        let widget: gtk::Widget = badge.upcast();
        tweaks.append(&check_row(
            if applied {
                State::Good
            } else {
                State::Advisory
            },
            tweak.title(),
            &format!("{}\n{}", tweak.subtitle(), tweak.change_text()),
            Some(&widget),
        ));
    }

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::Start);
    let needed = tune::tweaks_needed();
    let apply = gtk::Button::with_label(if needed.is_empty() {
        "All settings applied"
    } else {
        "Apply these settings"
    });
    apply.add_css_class("pill");
    apply.add_css_class("suggested-action");
    apply.set_sensitive(!needed.is_empty());
    apply.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| run_fixes(&app, vec![Fix::ApplyTweaks])
    ));
    buttons.append(&apply);
    if tune::tweaks_persisted() {
        let revert = gtk::Button::with_label("Restore the system's own settings");
        revert.add_css_class("pill");
        revert.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| confirm_revert(&app)
        ));
        buttons.append(&revert);
    }
    tweaks.append(&buttons);

    let persistence = gtk::Label::new(Some(if tune::tweaks_persisted() {
        "These are written to /etc/sysctl.d/99-raven-gaming.conf and /etc/security/limits.d/99-raven-gaming.conf, so they survive a reboot. The open-file limit takes effect at your next login."
    } else {
        "Applying writes one file in /etc/sysctl.d and one in /etc/security/limits.d, both named for this app, so undoing everything is deleting two files."
    }));
    persistence.set_xalign(0.0);
    persistence.set_wrap(true);
    persistence.add_css_class("note");
    tweaks.append(&persistence);
    page.append(&tweaks);

    // ---- shader cache ----
    page.append(&shader_cache_section(app));
    page_scroll(&page)
}

fn preset_card(
    app: &Rc<App>,
    system: &checks::System,
    preset: tune::Preset,
    active: bool,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("profile-card");
    if active {
        button.add_css_class("active-profile");
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    head.append(&glyph(preset.icon()));
    let name = gtk::Label::new(Some(preset.title()));
    name.add_css_class("row-title");
    head.append(&name);
    if active {
        let badge = gtk::Label::new(Some("Active"));
        badge.add_css_class("badge");
        badge.set_halign(gtk::Align::End);
        badge.set_hexpand(true);
        head.append(&badge);
    }
    content.append(&head);
    let description = gtk::Label::new(Some(preset.description()));
    description.set_wrap(true);
    description.set_xalign(0.0);
    description.add_css_class("dim-label");
    content.append(&description);
    button.set_child(Some(&content));

    // Applying a preset is two changes: the processor's profile, and every
    // card that has a clock policy. The card part needs root; the profile
    // part does not, so it is done first and its own failure reported
    // rather than being hidden behind a password prompt.
    let cards: Vec<String> = system
        .gpus
        .iter()
        .filter(|g| tune::gpu_mode_path(g).is_some())
        .filter_map(|g| g.card.clone())
        .collect();
    button.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| {
            match tune::set_power_profile(preset.power_profile()) {
                Ok(()) => app.toast(&format!("{} mode", preset.title())),
                Err(error) => {
                    app.toast(&error);
                    return;
                }
            }
            let actions: Vec<tune::Action> = cards
                .iter()
                .map(|card| tune::Action::GpuMode {
                    card: card.clone(),
                    mode: preset.gpu_mode(),
                })
                .collect();
            if actions.is_empty() {
                app.refresh();
            } else {
                run_root_actions(&app, &format!("{} mode", preset.title()), actions);
            }
        }
    ));
    button
}

fn gpu_mode_row(app: &Rc<App>, card_gpu: &Gpu) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");
    let text = section_title(
        &card_gpu.title(),
        match tune::gpu_mode(card_gpu) {
            Some(tune::GpuMode::High) => "Held at maximum clocks.",
            Some(tune::GpuMode::Low) => "Held at minimum clocks.",
            Some(tune::GpuMode::Auto) => "The driver is choosing the clocks.",
            None => "The current policy could not be read.",
        },
    );
    text.set_hexpand(true);
    row.append(&text);

    let segmented = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    segmented.add_css_class("segmented");
    segmented.add_css_class("linked");
    segmented.set_valign(gtk::Align::Center);
    let current = tune::gpu_mode(card_gpu);
    let mut first: Option<gtk::ToggleButton> = None;
    for mode in [tune::GpuMode::Low, tune::GpuMode::Auto, tune::GpuMode::High] {
        let toggle = gtk::ToggleButton::with_label(mode.title());
        toggle.set_active(current == Some(mode));
        match &first {
            Some(group) => toggle.set_group(Some(group)),
            None => first = Some(toggle.clone()),
        }
        let card_name = card_gpu.card.clone();
        toggle.connect_toggled(glib::clone!(
            #[strong]
            app,
            move |toggle| {
                // Only the button being switched on acts; the group also
                // fires for the one being switched off.
                if !toggle.is_active() || current == Some(mode) {
                    return;
                }
                let Some(card) = card_name.clone() else {
                    app.toast("This card has no clock policy to set");
                    return;
                };
                run_root_actions(
                    &app,
                    &format!("{} clocks", mode.title()),
                    vec![tune::Action::GpuMode { card, mode }],
                );
            }
        ));
        segmented.append(&toggle);
    }
    row.append(&segmented);
    row
}

fn shader_cache_section(app: &Rc<App>) -> gtk::Box {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 14);
    holder.append(&section_title(
        "Shader cache",
        "Compiled shaders, kept so the same effect does not have to be built twice. A cold cache is why the first few minutes of a new game stutter; a cache left over from an older driver is dead weight.",
    ));
    let list = card();
    let caches = tune::shader_cache_dirs();
    if caches.is_empty() {
        list.append(&check_row(
            State::Good,
            "Nothing cached yet",
            "No shader cache has been written. One appears the first time a game runs.",
            None,
        ));
    } else {
        for (path, size) in &caches {
            let clear = gtk::Button::with_label("Clear");
            clear.add_css_class("pill");
            clear.set_tooltip_text(Some(
                "Deletes the cached shaders. Games rebuild them, so the next run of each game will stutter once.",
            ));
            let target = path.clone();
            clear.connect_clicked(glib::clone!(
                #[strong]
                app,
                move |_| confirm_clear_cache(&app, target.clone())
            ));
            let widget: gtk::Widget = clear.upcast();
            list.append(&check_row(
                State::Good,
                &path.display().to_string(),
                &tune::human_bytes(*size),
                Some(&widget),
            ));
        }
    }
    holder.append(&list);
    holder
}

// ---- Games ---------------------------------------------------------------

fn games_page(app: &Rc<App>) -> gtk::Widget {
    let system = app.system();
    let page = page_box();
    let installed = |name: &str| system.has(name);
    let recipe = games::launch_recipe(&system.gpus, &installed);

    // ---- the launch options ----
    page.append(&section_title(
        "How to start a game on this computer",
        "Paste this into a game's launch options in Steam — right-click the game, Properties, Launch Options. It is built for the hardware in this machine.",
    ));
    let recipe_card = card();
    recipe_card.append(&copy_row(app, &recipe.steam_launch_options()));
    if recipe.notes.is_empty() {
        let note = gtk::Label::new(Some(
            "There is nothing this machine needs added: one graphics card, and no helpers installed. A game started plainly is already started correctly.",
        ));
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.add_css_class("dim-label");
        recipe_card.append(&note);
    } else {
        for note in &recipe.notes {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            line.add_css_class("bullet-row");
            let dot = glyph("emblem-ok-symbolic");
            dot.add_css_class("success");
            dot.set_valign(gtk::Align::Start);
            line.append(&dot);
            let text = gtk::Label::new(Some(note));
            text.set_xalign(0.0);
            text.set_wrap(true);
            text.set_hexpand(true);
            text.add_css_class("dim-label");
            line.append(&text);
            recipe_card.append(&line);
        }
    }
    if !recipe.shell_prefix().is_empty() {
        let terminal = gtk::Label::new(Some("Outside Steam, in a terminal:"));
        terminal.set_xalign(0.0);
        terminal.add_css_class("eyebrow");
        terminal.set_margin_top(6);
        recipe_card.append(&terminal);
        recipe_card.append(&copy_row(
            app,
            &format!("{} ./the-game", recipe.shell_prefix()),
        ));
    }
    page.append(&recipe_card);

    // ---- the library ----
    let library = games::discover();
    let configured = games::steam_launch_options();
    page.append(&section_title(
        "Installed games",
        &match library.len() {
            0 => "Nothing found yet.".to_string(),
            1 => "One game found.".to_string(),
            n => format!("{n} games found."),
        },
    ));
    if library.is_empty() {
        let none = card();
        none.append(&check_row(
            State::Advisory,
            "No games found",
            if games::steam_installed() {
                "Steam is installed but its library is empty. Anything installed through Steam, Lutris or Heroic shows up here."
            } else {
                "No game launcher is installed. Steam brings Proton with it, which is what runs Windows games on Linux; the Overview page can install it."
            },
            None,
        ));
        page.append(&none);
    } else {
        let list = card();
        for game in &library {
            list.append(&game_row(app, game, &configured, &recipe));
        }
        page.append(&list);
    }
    page_scroll(&page)
}

fn game_row(
    app: &Rc<App>,
    game: &games::Game,
    configured: &std::collections::BTreeMap<String, String>,
    recipe: &games::LaunchRecipe,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");

    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class(match game.source {
        games::Source::Steam => "blue",
        games::Source::Lutris => "orange",
        games::Source::Heroic => "purple",
    });
    tile.set_valign(gtk::Align::Start);
    tile.set_hexpand(false);
    let glyph = glyph("applications-games-symbolic");
    glyph.set_halign(gtk::Align::Center);
    glyph.set_hexpand(true);
    tile.append(&glyph);
    row.append(&tile);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&game.name));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.add_css_class("row-title");
    text.append(&name);
    let detail = gtk::Label::new(Some(&format!("{} · {}", game.source.name(), game.detail())));
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.add_css_class("dim-label");
    text.append(&detail);

    // What this game is actually set to run with, if anything.
    let current = game.app_id.as_ref().and_then(|id| configured.get(id));
    let wanted = recipe.steam_launch_options();
    let worth_setting = !recipe.is_empty();
    let status = match (current, game.source) {
        (Some(options), _) => {
            let label = gtk::Label::new(Some(&format!("Launch options: {options}")));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.add_css_class("mono");
            label.add_css_class("set-options");
            Some(label)
        }
        (None, games::Source::Steam) if worth_setting => {
            let label = gtk::Label::new(Some(
                "No launch options set — this one starts with the defaults.",
            ));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.add_css_class("dim-label");
            Some(label)
        }
        _ => None,
    };
    if let Some(label) = status {
        text.append(&label);
    }
    row.append(&text);

    if game.source == games::Source::Steam && worth_setting {
        let copy = gtk::Button::with_label("Copy options");
        copy.add_css_class("pill");
        copy.set_valign(gtk::Align::Center);
        copy.set_tooltip_text(Some(
            "Copies the recommended launch options; paste them into this game's properties in Steam",
        ));
        let wanted = wanted.clone();
        copy.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |button| {
                button.clipboard().set_text(&wanted);
                app.toast("Copied — paste into the game's Launch Options in Steam");
            }
        ));
        row.append(&copy);
    }
    row
}

// ---- Audio ---------------------------------------------------------------

fn audio_page(app: &Rc<App>) -> gtk::Widget {
    let page = page_box();
    let engine = audio::Engine::read();

    if !engine.running {
        return empty_state(
            "audio-speakers-symbolic",
            "PipeWire is not running",
            "Game audio, voice chat and the recorder all go through it. Without it there is nothing here to set.",
        );
    }

    // ---- latency ----
    let latency = engine.latency();
    let hero = card();
    hero.add_css_class("capture-hero");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 5);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&format!("Audio is {} behind", latency.text())));
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.add_css_class("panel-title");
    text.append(&title);
    let body = gtk::Label::new(Some(
        "PipeWire holds a buffer of sound before playing it. A big one never stutters and is what a desktop wants; a small one is what a game wants, because the buffer is the gap between the gun firing and the bang.",
    ));
    body.set_xalign(0.0);
    body.set_wrap(true);
    body.add_css_class("dim-label");
    text.append(&body);
    head.append(&text);
    let verdict = gtk::Label::new(Some(if latency.is_good_for_games() {
        "Good for games"
    } else {
        "Fine for a desktop"
    }));
    verdict.add_css_class("badge");
    verdict.add_css_class(if latency.is_good_for_games() {
        "installed"
    } else {
        "neutral"
    });
    verdict.set_valign(gtk::Align::Start);
    head.append(&verdict);
    hero.append(&head);
    page.append(&hero);

    // ---- the two controls ----
    page.append(&section_title(
        "Buffer and sample rate",
        "Applied to the running server the moment they change, so a buffer too small for this machine can be heard crackling and put back. Saving writes one file in your own config — no password, and deleting it restores PipeWire's defaults.",
    ));
    let controls = card();

    let quantum_labels: Vec<String> = std::iter::once("Automatic".to_string())
        .chain(audio::QUANTA.iter().map(|q| {
            let ms = *q as f64 / engine.rate.max(1) as f64 * 1000.0;
            format!("{q} frames · {ms:.1} ms")
        }))
        .collect();
    let quantum_row = dropdown_row(
        "Buffer size",
        "Smaller is tighter. 256 frames suits most machines from the last decade; below that is for people who can hear the difference and can put up with the risk.",
        &quantum_labels,
        if engine.forced_quantum == 0 {
            0
        } else {
            audio::QUANTA
                .iter()
                .position(|q| *q == engine.forced_quantum)
                .map(|i| i + 1)
                .unwrap_or(0)
        },
    );
    quantum_row.1.connect_selected_notify(glib::clone!(
        #[strong]
        app,
        move |dropdown| {
            let chosen = match dropdown.selected() {
                0 => 0,
                index => audio::QUANTA
                    .get(index as usize - 1)
                    .copied()
                    .unwrap_or_default(),
            };
            if chosen == engine.forced_quantum {
                return;
            }
            match audio::apply_quantum(chosen) {
                Ok(()) => {
                    app.toast(if chosen == 0 {
                        "Buffer back to automatic"
                    } else {
                        "Buffer changed — listen for crackling before saving"
                    });
                    app.refresh();
                }
                Err(error) => app.toast(&error),
            }
        }
    ));
    controls.append(&quantum_row.0);

    let rate_labels: Vec<String> = std::iter::once("Automatic".to_string())
        .chain(
            audio::RATES
                .iter()
                .map(|r| format!("{} kHz", *r as f64 / 1000.0)),
        )
        .collect();
    let rate_row = dropdown_row(
        "Sample rate",
        "48 kHz is what games and PipeWire both assume. Forcing a rate stops PipeWire resampling, which is worth doing only if you know the hardware's own rate.",
        &rate_labels,
        if engine.forced_rate == 0 {
            0
        } else {
            audio::RATES
                .iter()
                .position(|r| *r == engine.forced_rate)
                .map(|i| i + 1)
                .unwrap_or(0)
        },
    );
    rate_row.1.connect_selected_notify(glib::clone!(
        #[strong]
        app,
        move |dropdown| {
            let chosen = match dropdown.selected() {
                0 => 0,
                index => audio::RATES
                    .get(index as usize - 1)
                    .copied()
                    .unwrap_or_default(),
            };
            if chosen == engine.forced_rate {
                return;
            }
            match audio::apply_rate(chosen) {
                Ok(()) => {
                    app.toast("Sample rate changed");
                    app.refresh();
                }
                Err(error) => app.toast(&error),
            }
        }
    ));
    controls.append(&rate_row.0);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::Start);
    let save = gtk::Button::with_label("Keep these after a reboot");
    save.add_css_class("pill");
    save.add_css_class("suggested-action");
    save.set_sensitive(engine.forced_quantum != 0 || engine.forced_rate != 0);
    let (forced_quantum, forced_rate) = (engine.forced_quantum, engine.forced_rate);
    save.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| match audio::persist(forced_quantum, forced_rate) {
            Ok(()) => {
                app.toast("Saved");
                app.refresh();
            }
            Err(error) => app.toast(&error),
        }
    ));
    buttons.append(&save);
    if audio::is_persisted() || engine.forced_quantum != 0 || engine.forced_rate != 0 {
        let restore = gtk::Button::with_label("Restore PipeWire's own settings");
        restore.add_css_class("pill");
        restore.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| match audio::restore() {
                Ok(()) => {
                    app.toast("Restored");
                    app.refresh();
                }
                Err(error) => app.toast(&error),
            }
        ));
        buttons.append(&restore);
    }
    controls.append(&buttons);
    if audio::is_persisted() {
        let note = gtk::Label::new(Some(
            "Saved to ~/.config/pipewire/pipewire.conf.d/99-raven-gaming.conf. Deleting that file is the same as pressing Restore.",
        ));
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.add_css_class("note");
        controls.append(&note);
    }
    page.append(&controls);

    // ---- devices ----
    let devices = audio::devices();
    page.append(&section_title(
        "Where the sound goes",
        "A game follows the system default unless it has been told otherwise.",
    ));
    let list = card();
    if devices.is_empty() {
        list.append(&check_row(
            State::Unknown,
            "No devices found",
            "PipeWire is running but reported no sinks or sources. pw-dump is what this page asks; without it there is nothing to show.",
            None,
        ));
    }
    for device in &devices {
        list.append(&audio_device_row(app, device));
    }
    page.append(&list);

    // ---- the 32-bit trap ----
    page.append(&section_title("Sound in 32-bit games", ""));
    let bits = card();
    let has32 = audio::has_32bit_audio();
    let audio32_fix = fix_button(
        app,
        &Fix::Install(vec!["lib32-pipewire".into(), "lib32-libpulse".into()]),
    );
    bits.append(&check_row(
        if has32 { State::Good } else { State::Problem },
        "32-bit audio libraries",
        if has32 {
            "Installed, so Proton and older native games can reach the sound server."
        } else {
            "Missing. A 32-bit or Proton game will run perfectly and in total silence, and nothing in the game's own settings explains why."
        },
        if has32 { None } else { audio32_fix.as_ref() },
    ));
    page.append(&bits);
    page_scroll(&page)
}

/// A labelled row with a dropdown on the right.
fn dropdown_row(
    title: &str,
    subtitle: &str,
    options: &[String],
    selected: usize,
) -> (gtk::Box, gtk::DropDown) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    row.add_css_class("data-row");
    let text = section_title(title, subtitle);
    text.set_hexpand(true);
    row.append(&text);
    let labels: Vec<&str> = options.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::new(Some(gtk::StringList::new(&labels)), gtk::Expression::NONE);
    dropdown.set_valign(gtk::Align::Center);
    dropdown.set_selected(selected as u32);
    row.append(&dropdown);
    (row, dropdown)
}

fn audio_device_row(app: &Rc<App>, device: &audio::Device) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");
    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class(if device.is_output { "blue" } else { "teal" });
    tile.set_valign(gtk::Align::Start);
    tile.set_hexpand(false);
    let image = glyph(if device.is_output {
        "audio-speakers-symbolic"
    } else {
        "audio-input-microphone-symbolic"
    });
    image.set_halign(gtk::Align::Center);
    image.set_hexpand(true);
    tile.append(&image);
    row.append(&tile);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&device.description));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    name.add_css_class("row-title");
    text.append(&name);
    let detail = gtk::Label::new(Some(&format!(
        "{} · {}",
        if device.is_output { "Output" } else { "Input" },
        if device.api.is_empty() {
            "PipeWire"
        } else {
            &device.api
        }
    )));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    text.append(&detail);
    row.append(&text);

    if device.is_default {
        let badge = gtk::Label::new(Some("Default"));
        badge.add_css_class("badge");
        badge.set_valign(gtk::Align::Center);
        row.append(&badge);
    } else {
        let use_it = gtk::Button::with_label("Use this");
        use_it.add_css_class("pill");
        use_it.set_valign(gtk::Align::Center);
        let target = device.clone();
        use_it.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| match audio::set_default(&target) {
                Ok(()) => {
                    app.toast(&format!("Now using {}", target.description));
                    app.refresh();
                }
                Err(error) => app.toast(&error),
            }
        ));
        row.append(&use_it);
    }
    row
}

// ---- Controllers ---------------------------------------------------------

fn controllers_page(app: &Rc<App>) -> gtk::Widget {
    let page = page_box();
    let pads = controllers::discover();

    // ---- the system's side, whether or not anything is plugged in ----
    let rules = controllers::steam_input_rules();
    let rules_fix = fix_button(app, &Fix::Install(vec!["steam-devices".into()]));
    let permissions = card();
    permissions.append(&check_row(
        if rules { State::Good } else { State::Problem },
        "Controller permissions",
        if rules {
            "The steam-devices udev rules are installed, so any game can open a controller — not only Steam."
        } else {
            "The steam-devices udev rules are missing. Controllers work inside Steam, which runs its own helper, and are invisible to everything else — which almost nobody attributes to a missing package."
        },
        if rules { None } else { rules_fix.as_ref() },
    ));
    page.append(&permissions);

    if pads.is_empty() {
        page.append(&section_title("Nothing connected", ""));
        let none = card();
        none.append(&check_row(
            State::Advisory,
            "No controller found",
            "Plug one in over USB, or pair it over Bluetooth, and it appears here. Xbox, PlayStation, Switch Pro and most third-party pads are driven by the kernel with nothing to install.

Keyboard and mouse work for everything on the Library page; a pad is only required by the emulators.",
            None,
        ));
        page.append(&none);
        return page_scroll(&page);
    }

    page.append(&section_title(
        &match pads.len() {
            1 => "One controller".to_string(),
            n => format!("{n} controllers"),
        },
        "Press Test to watch the buttons and sticks live — if this window can read them, so can a game.",
    ));
    for pad in &pads {
        page.append(&controller_card(app, pad));
    }
    page_scroll(&page)
}

fn controller_card(app: &Rc<App>, pad: &controllers::Controller) -> gtk::Box {
    let holder = card();
    holder.add_css_class("gpu-card");

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class("large");
    tile.add_css_class(match pad.transport {
        controllers::Transport::Bluetooth => "indigo",
        controllers::Transport::Usb => "green",
        _ => "gray",
    });
    tile.set_valign(gtk::Align::Start);
    tile.set_hexpand(false);
    let image = glyph("input-gaming-symbolic");
    image.set_halign(gtk::Align::Center);
    image.set_hexpand(true);
    tile.append(&image);
    head.append(&tile);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&pad.name));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    name.add_css_class("gpu-name");
    text.append(&name);
    let mut facts = vec![
        pad.kind().to_string(),
        pad.transport.name().to_string(),
        format!("{:04x}:{:04x}", pad.vendor, pad.product),
        format!("{} buttons · {} axes", pad.buttons, pad.axes),
    ];
    if pad.has_rumble {
        facts.push("rumble".into());
    }
    if let Some(driver) = pad.driver() {
        facts.push(format!("driver {driver}"));
    }
    let detail = gtk::Label::new(Some(&facts.join(" · ")));
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    detail.add_css_class("dim-label");
    text.append(&detail);
    head.append(&text);

    if let Some(percent) = pad.battery() {
        let battery = gtk::Label::new(Some(&format!("{percent}%")));
        battery.add_css_class("badge");
        battery.add_css_class(if percent < 20 { "warning" } else { "installed" });
        battery.set_valign(gtk::Align::Start);
        head.append(&battery);
    }
    holder.append(&head);

    let readable = controllers::can_read(pad);
    holder.append(&check_row(
        if readable { State::Good } else { State::Problem },
        "Readable by games",
        &match &pad.event {
            Some(path) if readable => format!(
                "{} can be opened by this session, which is exactly what a game does.",
                path.display()
            ),
            Some(path) => format!(
                "{} cannot be opened. Being in the `input` group, or having the steam-devices rules installed, is what grants this.",
                path.display()
            ),
            None => "This device has no event node, so nothing can read it.".to_string(),
        },
        None,
    ));

    if readable {
        let test = gtk::Button::with_label("Test this controller");
        test.add_css_class("pill");
        test.add_css_class("suggested-action");
        test.set_halign(gtk::Align::Start);
        let target = pad.clone();
        test.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| open_tester(&app, &target)
        ));
        holder.append(&test);
    }
    holder
}

/// A window that shows what the controller is doing, right now.
fn open_tester(app: &Rc<App>, pad: &controllers::Controller) {
    let mut reader = match controllers::Reader::open(pad) {
        Ok(reader) => reader,
        Err(error) => {
            let dialog = adw::AlertDialog::new(Some("Cannot read this controller"), Some(&error));
            dialog.add_response("ok", "OK");
            dialog.present(Some(&app.window));
            return;
        }
    };

    let dialog = adw::Dialog::new();
    dialog.set_title(&pad.name);
    dialog.set_content_width(560);
    dialog.set_content_height(460);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
    body.set_margin_start(20);
    body.set_margin_end(20);
    body.set_margin_top(4);
    body.set_margin_bottom(20);

    let lede = gtk::Label::new(Some(
        "Press every button and push both sticks to their corners. Anything that does not light up here will not work in a game either.",
    ));
    lede.set_xalign(0.0);
    lede.set_wrap(true);
    lede.add_css_class("dim-label");
    body.append(&lede);

    let last = gtk::Label::new(Some("Waiting…"));
    last.set_xalign(0.0);
    last.add_css_class("panel-title");
    body.append(&last);

    let held_label = gtk::Label::new(Some("Nothing held"));
    held_label.set_xalign(0.0);
    held_label.set_wrap(true);
    held_label.add_css_class("dim-label");
    body.append(&held_label);

    let axes_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&axes_box)
        .vexpand(true)
        .build();
    body.append(&scroller);
    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(&app.window));

    // Axis bars are made as axes are first seen, so only the ones this
    // controller actually has appear.
    let bars: Rc<RefCell<std::collections::BTreeMap<u16, (gtk::LevelBar, gtk::Label)>>> =
        Rc::new(RefCell::new(std::collections::BTreeMap::new()));
    let held: Rc<RefCell<std::collections::BTreeSet<u16>>> =
        Rc::new(RefCell::new(std::collections::BTreeSet::new()));
    // Axis ranges are per-device and not published in /proc, so the tester
    // learns them: the extremes seen so far are the extremes of the bar.
    let range: Rc<RefCell<std::collections::BTreeMap<u16, (i32, i32)>>> =
        Rc::new(RefCell::new(std::collections::BTreeMap::new()));

    let alive = Rc::new(Cell::new(true));
    dialog.connect_closed(glib::clone!(
        #[strong]
        alive,
        move |_| alive.set(false)
    ));

    glib::timeout_add_local(Duration::from_millis(16), move || {
        if !alive.get() {
            return glib::ControlFlow::Break;
        }
        for event in reader.poll() {
            match event {
                controllers::Input::Button { code, pressed } => {
                    let name = controllers::button_name(code);
                    if pressed {
                        held.borrow_mut().insert(code);
                        last.set_text(&name);
                    } else {
                        held.borrow_mut().remove(&code);
                    }
                    let names: Vec<String> = held
                        .borrow()
                        .iter()
                        .map(|c| controllers::button_name(*c))
                        .collect();
                    held_label.set_text(&if names.is_empty() {
                        "Nothing held".to_string()
                    } else {
                        format!("Held: {}", names.join(", "))
                    });
                }
                controllers::Input::Axis { code, value } => {
                    let mut ranges = range.borrow_mut();
                    let seen = ranges.entry(code).or_insert((value, value));
                    seen.0 = seen.0.min(value);
                    seen.1 = seen.1.max(value);
                    let (low, high) = *seen;
                    drop(ranges);
                    let mut bars = bars.borrow_mut();
                    let entry = bars.entry(code).or_insert_with(|| {
                        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
                        let caption = gtk::Label::new(Some(&controllers::axis_name(code)));
                        caption.set_xalign(0.0);
                        caption.add_css_class("meter-name");
                        row.append(&caption);
                        let bar = gtk::LevelBar::new();
                        bar.set_min_value(0.0);
                        bar.set_max_value(1.0);
                        row.append(&bar);
                        axes_box.append(&row);
                        (bar, caption)
                    });
                    let span = (high - low).max(1) as f64;
                    entry
                        .0
                        .set_value(((value - low) as f64 / span).clamp(0.0, 1.0));
                    entry
                        .1
                        .set_text(&format!("{} — {value}", controllers::axis_name(code)));
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

// ---- Emulators -----------------------------------------------------------

fn emulators_page(app: &Rc<App>) -> gtk::Widget {
    let system = app.system();
    let page = page_box();
    let pads = controllers::discover().len();

    page.append(&section_title(
        "Before anything will run",
        "Three things every emulator wants from the machine. Each one is dealt with on another page.",
    ));
    let needs = card();
    for prerequisite in emulators::prerequisites(&system.gpus, pads) {
        let go = gtk::Button::with_label("Open");
        go.add_css_class("pill");
        let target = prerequisite.page;
        go.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| jump_to(&app, target)
        ));
        let trailing: Option<gtk::Widget> = if prerequisite.met {
            None
        } else {
            Some(go.upcast())
        };
        needs.append(&check_row(
            if prerequisite.met {
                State::Good
            } else {
                State::Advisory
            },
            prerequisite.title,
            &prerequisite.detail,
            trailing.as_ref(),
        ));
    }
    page.append(&needs);

    let note = gtk::Label::new(Some(
        "Raven Gaming installs emulators. It does not supply BIOS images, firmware or keys — those come from hardware you own, and where one is needed it is said below.",
    ));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("info-note");
    page.append(&note);

    let installed = emulators::installed().len();
    page.append(&section_title(
        "Emulators",
        &match installed {
            0 => "None installed yet. Each one below is a single package.".to_string(),
            1 => "One installed.".to_string(),
            n => format!("{n} installed."),
        },
    ));
    for family in emulators::FAMILIES {
        page.append(&section_title(family.title(), ""));
        let shelf = card();
        for emulator in emulators::by_family(family) {
            shelf.append(&emulator_row(app, emulator, &system));
        }
        page.append(&shelf);
    }
    page_scroll(&page)
}

fn emulator_row(
    app: &Rc<App>,
    emulator: &'static emulators::Emulator,
    system: &checks::System,
) -> gtk::Box {
    let installed = emulator.is_installed() || system.has(emulator.package);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");

    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class(emulator.family.tint());
    tile.set_valign(gtk::Align::Start);
    tile.set_hexpand(false);
    let image = glyph("applications-system-symbolic");
    image.set_halign(gtk::Align::Center);
    image.set_hexpand(true);
    tile.append(&image);
    row.append(&tile);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(emulator.name));
    name.set_xalign(0.0);
    name.add_css_class("row-title");
    text.append(&name);
    let systems = gtk::Label::new(Some(emulator.systems));
    systems.set_xalign(0.0);
    systems.set_wrap(true);
    systems.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    systems.add_css_class("dim-label");
    text.append(&systems);
    if let Some(caveat) = emulator.caveat {
        let warning = gtk::Label::new(Some(caveat));
        warning.set_xalign(0.0);
        warning.set_wrap(true);
        warning.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        warning.add_css_class("dim-label");
        warning.add_css_class("warning");
        text.append(&warning);
    }
    row.append(&text);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_valign(gtk::Align::Center);
    if installed {
        if emulator.is_installed() {
            let launch = gtk::Button::with_label("Open");
            launch.add_css_class("pill");
            let binary = emulator.binary;
            launch.connect_clicked(glib::clone!(
                #[strong]
                app,
                move |_| match run_detached(binary, &[]) {
                    Ok(()) => app.toast(&format!("Starting {}", emulator.name)),
                    Err(error) => app.toast(&error),
                }
            ));
            buttons.append(&launch);
        }
        let badge = gtk::Label::new(Some("Installed"));
        badge.add_css_class("badge");
        badge.add_css_class("installed");
        badge.set_valign(gtk::Align::Center);
        buttons.append(&badge);
    } else {
        let install = gtk::Button::with_label("Install");
        install.add_css_class("pill");
        install.add_css_class("suggested-action");
        let package = emulator.package.to_string();
        install.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| run_fixes(&app, vec![Fix::Install(vec![package.clone()])])
        ));
        buttons.append(&install);
    }
    row.append(&buttons);
    row
}

// ---- Game tools ----------------------------------------------------------

fn tools_page(app: &Rc<App>) -> gtk::Widget {
    let system = app.system();
    let page = page_box();
    let packages = |name: &str| system.has(name);

    for kind in tools::KINDS {
        page.append(&section_title(kind.title(), kind.lede()));
        let shelf = card();
        let mut missing: Vec<String> = Vec::new();
        for tool in tools::by_kind(kind) {
            if !tool.is_installed(&packages) {
                missing.push(tool.package.to_string());
            }
            shelf.append(&tool_row(app, tool, &packages));
        }
        if missing.len() > 1 {
            let all = gtk::Button::with_label(&format!("Install all {}", missing.len()));
            all.add_css_class("pill");
            all.add_css_class("suggested-action");
            all.set_halign(gtk::Align::Start);
            all.connect_clicked(glib::clone!(
                #[strong]
                app,
                move |_| run_fixes(&app, vec![Fix::Install(missing.clone())])
            ));
            shelf.append(&all);
        }
        page.append(&shelf);
    }

    // ---- probes ----
    page.append(&section_title(
        "Ask the system directly",
        "Each of these runs one read-only command and shows you what it said.",
    ));
    let probes = card();
    for probe in tools::PROBES {
        let run = gtk::Button::with_label("Run");
        run.add_css_class("pill");
        run.set_sensitive(probe.is_available());
        run.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| show_probe(&app, probe)
        ));
        let trailing: gtk::Widget = run.upcast();
        probes.append(&check_row(
            if probe.is_available() {
                State::Good
            } else {
                State::Advisory
            },
            probe.name,
            &if probe.is_available() {
                probe.what.to_string()
            } else {
                format!("{} — {} is not installed.", probe.what, probe.binary)
            },
            Some(&trailing),
        ));
    }
    page.append(&probes);

    // ---- Proton ----
    page.append(&proton_section(app));
    page_scroll(&page)
}

fn tool_row(
    app: &Rc<App>,
    tool: &'static tools::Tool,
    packages: &dyn Fn(&str) -> bool,
) -> gtk::Box {
    let installed = tool.is_installed(packages);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");

    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class(tool.kind.tint());
    tile.set_valign(gtk::Align::Start);
    tile.set_hexpand(false);
    let image = glyph("applications-utilities-symbolic");
    image.set_halign(gtk::Align::Center);
    image.set_hexpand(true);
    tile.append(&image);
    row.append(&tile);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(tool.name));
    name.set_xalign(0.0);
    name.add_css_class("row-title");
    text.append(&name);
    let what = gtk::Label::new(Some(tool.what));
    what.set_xalign(0.0);
    what.set_wrap(true);
    what.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    what.add_css_class("dim-label");
    text.append(&what);
    row.append(&text);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_valign(gtk::Align::Center);
    if installed {
        if tool.launchable && drivers::which(tool.binary).is_some() {
            let open = gtk::Button::with_label("Open");
            open.add_css_class("pill");
            let binary = tool.binary;
            open.connect_clicked(glib::clone!(
                #[strong]
                app,
                move |_| match run_detached(binary, &[]) {
                    Ok(()) => app.toast(&format!("Starting {}", tool.name)),
                    Err(error) => app.toast(&error),
                }
            ));
            buttons.append(&open);
        }
        let badge = gtk::Label::new(Some("Installed"));
        badge.add_css_class("badge");
        badge.add_css_class("installed");
        badge.set_valign(gtk::Align::Center);
        buttons.append(&badge);
    } else {
        let install = gtk::Button::with_label("Install");
        install.add_css_class("pill");
        let package = tool.package.to_string();
        install.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| run_fixes(&app, vec![Fix::Install(vec![package.clone()])])
        ));
        buttons.append(&install);
    }
    row.append(&buttons);
    row
}

fn show_probe(app: &Rc<App>, probe: tools::Probe) {
    let dialog = adw::Dialog::new();
    dialog.set_title(probe.name);
    dialog.set_content_width(760);
    dialog.set_content_height(560);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_start(20);
    body.set_margin_end(20);
    body.set_margin_top(4);
    body.set_margin_bottom(20);

    let text = match probe.run() {
        Ok(output) => output,
        Err(error) => format!(
            "{} did not answer.

{error}",
            probe.binary
        ),
    };
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(&text);
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_left_margin(6);
    view.set_right_margin(6);
    view.add_css_class("task-log");
    let scroller = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .build();
    body.append(&scroller);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let copy = gtk::Button::with_label("Copy");
    copy.add_css_class("pill");
    copy.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |button| {
            button.clipboard().set_text(&text);
            app.toast("Copied");
        }
    ));
    buttons.append(&copy);
    let close = gtk::Button::with_label("Close");
    close.add_css_class("pill");
    close.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    buttons.append(&close);
    body.append(&buttons);

    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(&app.window));
}

fn proton_section(app: &Rc<App>) -> gtk::Box {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 14);
    let builds = tools::proton_builds();
    holder.append(&section_title(
        "Proton",
        "The translation layer that runs Windows games, and the prefix each game keeps its Windows-side files in.",
    ));

    let list = card();
    if builds.is_empty() {
        list.append(&check_row(
            State::Advisory,
            "No Proton builds",
            "Steam downloads one the first time a Windows game is launched. A build installed by hand goes in compatibilitytools.d.",
            None,
        ));
    }
    for build in &builds {
        list.append(&check_row(
            State::Good,
            &build.name,
            if build.official {
                "From Steam."
            } else {
                "Installed by hand into compatibilitytools.d."
            },
            None,
        ));
    }
    holder.append(&list);

    // Prefixes, named after the games they belong to.
    let names: std::collections::BTreeMap<String, String> = games::discover()
        .into_iter()
        .filter_map(|game| Some((game.app_id?, game.name)))
        .collect();
    let prefixes = tools::prefixes(&names);
    if prefixes.is_empty() {
        return holder;
    }
    holder.append(&section_title(
        "Game prefixes",
        "Each game's own C: drive — its settings, its saves and the Windows runtimes installed into it. Deleting one makes Proton build it again from scratch, which is the oldest fix there is, and takes the saves in it with them.",
    ));
    let prefix_list = card();
    for prefix in &prefixes {
        prefix_list.append(&prefix_row(app, prefix));
    }
    holder.append(&prefix_list);
    holder
}

fn prefix_row(app: &Rc<App>, prefix: &tools::Prefix) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&prefix.title()));
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    title.add_css_class("row-title");
    text.append(&title);
    let detail = gtk::Label::new(Some(&format!(
        "{} · app {}{}",
        tune::human_bytes(prefix.bytes),
        prefix.app_id,
        if prefix.game.is_none() {
            " · the game is no longer installed"
        } else {
            ""
        }
    )));
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.add_css_class("dim-label");
    text.append(&detail);
    row.append(&text);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_valign(gtk::Align::Center);
    if drivers::which("protontricks").is_some() {
        let tricks = gtk::Button::with_label("Protontricks");
        tricks.add_css_class("pill");
        let app_id = prefix.app_id.clone();
        tricks.connect_clicked(glib::clone!(
            #[strong]
            app,
            move |_| match tools::protontricks(&app_id) {
                Ok(()) => app.toast("Opening protontricks"),
                Err(error) => app.toast(&error),
            }
        ));
        buttons.append(&tricks);
    }
    let open = gtk::Button::from_icon_name(&icon_name("folder-open-symbolic"));
    open.add_css_class("flat");
    open.set_tooltip_text(Some("Show in the file manager"));
    let path = prefix.path.clone();
    open.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| {
            if let Err(error) = capture::open_in_file_manager(&path) {
                app.toast(&error);
            }
        }
    ));
    buttons.append(&open);

    let delete = gtk::Button::from_icon_name(&icon_name("user-trash-symbolic"));
    delete.add_css_class("flat");
    delete.set_tooltip_text(Some("Delete this prefix"));
    let doomed = prefix.clone();
    delete.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| confirm_delete_prefix(&app, doomed.clone())
    ));
    buttons.append(&delete);
    row.append(&buttons);
    row
}

fn confirm_delete_prefix(app: &Rc<App>, prefix: tools::Prefix) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Delete the prefix for {}?", prefix.title())),
        Some(&format!(
            "{} is removed for good. Proton builds a new one the next time the game starts, which resolves a lot of things that have gone wrong — and loses every setting, every installed runtime, and any save the game kept inside the prefix rather than in the cloud.",
            tune::human_bytes(prefix.bytes)
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("open", "Show me first");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        glib::clone!(
            #[strong]
            app,
            move |_, response| match response {
                "open" => {
                    if let Err(error) = capture::open_in_file_manager(&prefix.path) {
                        app.toast(&error);
                    }
                }
                "delete" => {
                    // Checked again on the way out: the dialog has been
                    // open, and the cost of being wrong is somebody's
                    // files.
                    if !tools::is_deletable_prefix(&prefix.path) {
                        app.toast("That is not a Proton prefix, so it was not touched");
                        return;
                    }
                    match std::fs::remove_dir_all(&prefix.path) {
                        Ok(()) => {
                            app.toast("Prefix deleted");
                            app.refresh();
                        }
                        Err(error) => app.toast(&format!("Could not delete it: {error}")),
                    }
                }
                _ => {}
            }
        ),
    );
    dialog.present(Some(&app.window));
}

// ---- Capture -------------------------------------------------------------

fn capture_page(app: &Rc<App>) -> gtk::Widget {
    let page = page_box();

    // ---- how to record ----
    let hero = card();
    hero.add_css_class("capture-hero");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("record-dot");
    dot.add_css_class("idle");
    dot.set_valign(gtk::Align::Center);
    head.append(&dot);
    let head_text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    head_text.set_hexpand(true);
    let title = gtk::Label::new(Some("Recording is built into the desktop"));
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.add_css_class("panel-title");
    head_text.append(&title);
    let body = gtk::Label::new(Some(
        "Huginn records the screen itself, inside the render loop, so a recording cannot be dropped by a capture program that misses a frame. Press the shortcut to start, press it again to stop.",
    ));
    body.set_xalign(0.0);
    body.set_wrap(true);
    body.add_css_class("dim-label");
    head_text.append(&body);
    head.append(&head_text);
    let chord = keys(&share::RECORD_SHORTCUT);
    chord.set_valign(gtk::Align::Center);
    head.append(&chord);
    hero.append(&head);

    let live_line = gtk::Label::new(Some("Not recording."));
    live_line.set_xalign(0.0);
    live_line.add_css_class("dim-label");
    // The dot only glows while something is actually being recorded. A
    // permanently lit record light is the kind of small lie that makes
    // people stop trusting the rest of the window.
    let dot_handle = dot.clone();
    app.live.bind(move |r| {
        if r.recording {
            dot_handle.remove_css_class("idle");
        } else {
            dot_handle.add_css_class("idle");
        }
    });
    app.live.label(&live_line, |r| {
        if r.recording {
            "Recording now.".into()
        } else {
            "Not recording.".into()
        }
    });
    hero.append(&live_line);
    page.append(&hero);

    // ---- the recordings ----
    let recordings = capture::recordings();
    let folder = capture::recordings_dir();
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let heading_text = section_title("Your recordings", &format!("In {}", folder.display()));
    heading_text.set_hexpand(true);
    heading.append(&heading_text);
    let open = gtk::Button::with_label("Open folder");
    open.add_css_class("pill");
    open.set_valign(gtk::Align::Center);
    open.set_sensitive(folder.is_dir());
    open.connect_clicked(glib::clone!(
        #[strong]
        app,
        #[strong]
        folder,
        move |_| {
            if let Err(error) = capture::open_in_file_manager(&folder) {
                app.toast(&error);
            }
        }
    ));
    heading.append(&open);
    page.append(&heading);

    if recordings.is_empty() {
        let none = card();
        none.append(&check_row(
            State::Advisory,
            "Nothing recorded yet",
            &format!(
                "Press Super+Print while a game is on screen. The recording lands in {} and appears here.",
                folder.display()
            ),
            None,
        ));
        page.append(&none);
    } else {
        let list = card();
        for recording in &recordings {
            list.append(&recording_row(app, recording));
        }
        page.append(&list);
    }

    // ---- exporting ----
    page.append(&section_title(
        "Turning a recording into a video",
        "Recordings are saved in Raven's own lossless format, which is cheap to write while a game is running and which no video player opens. Exporting makes an MP4 that anything plays.",
    ));
    let export_card = card();
    if capture::exporter_available() {
        export_card.append(&check_row(
            State::Good,
            "raven-export is installed",
            "Every recording above has an Export button. The MP4 is written beside the recording, and the recording is kept.",
            None,
        ));
    } else {
        export_card.append(&check_row(
            State::Problem,
            "raven-export is not installed",
            "It ships with the Raven desktop and is what turns a recording into a video. Without it a recording can still be made and kept, but not played anywhere else.",
            None,
        ));
    }
    page.append(&export_card);
    page_scroll(&page)
}

fn recording_row(app: &Rc<App>, recording: &capture::Recording) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("data-row");

    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class(if recording.complete { "red" } else { "orange" });
    tile.set_valign(gtk::Align::Start);
    tile.set_hexpand(false);
    let glyph = glyph("media-record-symbolic");
    glyph.set_halign(gtk::Align::Center);
    glyph.set_hexpand(true);
    tile.append(&glyph);
    row.append(&tile);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&recording.name()));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.add_css_class("row-title");
    text.append(&name);
    let detail = gtk::Label::new(Some(&recording.detail()));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    text.append(&detail);
    if let Some(mp4) = &recording.exported {
        let exported = gtk::Label::new(Some(&format!(
            "Exported to {}",
            mp4.file_name().unwrap_or_default().to_string_lossy()
        )));
        exported.set_xalign(0.0);
        exported.add_css_class("dim-label");
        exported.add_css_class("success");
        text.append(&exported);
    }
    row.append(&text);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_valign(gtk::Align::Center);
    let export = gtk::Button::with_label(if recording.exported.is_some() {
        "Export again"
    } else {
        "Export"
    });
    export.add_css_class("pill");
    if recording.exported.is_none() {
        export.add_css_class("suggested-action");
    }
    export.set_sensitive(capture::exporter_available());
    if !capture::exporter_available() {
        export.set_tooltip_text(Some("raven-export is not installed"));
    }
    let source = recording.path.clone();
    let destination = recording.export_path();
    export.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| run_export(&app, source.clone(), destination.clone())
    ));
    buttons.append(&export);

    let reveal = gtk::Button::from_icon_name(&icon_name("folder-open-symbolic"));
    reveal.add_css_class("flat");
    reveal.set_tooltip_text(Some("Show in the file manager"));
    let path = recording.path.clone();
    reveal.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| {
            let target = path.parent().unwrap_or(&path).to_path_buf();
            if let Err(error) = capture::open_in_file_manager(&target) {
                app.toast(&error);
            }
        }
    ));
    buttons.append(&reveal);

    let delete = gtk::Button::from_icon_name(&icon_name("user-trash-symbolic"));
    delete.add_css_class("flat");
    delete.set_tooltip_text(Some("Delete this recording"));
    let doomed = recording.path.clone();
    let title = recording.name();
    delete.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| confirm_delete_recording(&app, doomed.clone(), title.clone())
    ));
    buttons.append(&delete);
    row.append(&buttons);
    row
}

// ---- Screen sharing ------------------------------------------------------

fn sharing_page(app: &Rc<App>) -> gtk::Widget {
    let page = page_box();
    let list = share::checks();
    let blocked = list
        .iter()
        .any(|c| c.title == "Screen-sharing backend" && c.state == State::Problem);

    if blocked && share::compositor_is_huginn() {
        let banner = card();
        banner.add_css_class("alert-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let icon = glyph("dialog-warning-symbolic");
        icon.add_css_class("alert-icon");
        icon.set_valign(gtk::Align::Start);
        icon.set_pixel_size(22);
        head.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
        text.set_hexpand(true);
        let title = gtk::Label::new(Some(
            "Sharing a screen into a call does not work on this desktop yet",
        ));
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.add_css_class("row-title");
        text.append(&title);
        let body = gtk::Label::new(Some(
            "A screen share on Wayland needs three things: an application asking the desktop portal, a portal backend, and a compositor that will hand over frames. Huginn draws the screen and records it itself, and offers no protocol for a backend to capture through — so there is no package to install that would make this work. It needs support in the compositor.\n\nWhat does work is recording: Super+Print captures the screen at full quality, and the Capture page turns a recording into a video you can send. Some applications can also share a single window of their own without going near the portal.",
        ));
        body.set_xalign(0.0);
        body.set_wrap(true);
        body.add_css_class("dim-label");
        text.append(&body);
        head.append(&text);
        banner.append(&head);
        let go = gtk::Button::with_label("Go to Capture");
        go.add_css_class("pill");
        go.set_halign(gtk::Align::Start);
        go.connect_clicked(glib::clone!(
            #[weak(rename_to = stack)]
            app.stack,
            move |_| stack.set_visible_child_name("capture")
        ));
        banner.append(&go);
        page.append(&banner);
    }

    page.append(&section_title(
        "What a screen share needs",
        "Each piece, and whether this computer has it.",
    ));
    let card_list = card();
    for check in &list {
        let detail = match &check.fix {
            Some(Fix::Manual(advice)) => format!("{}\n\n{advice}", check.detail),
            _ => check.detail.clone(),
        };
        let trailing = check.fix.as_ref().and_then(|fix| fix_button(app, fix));
        card_list.append(&check_row(
            check.state,
            &check.title,
            &detail,
            trailing.as_ref(),
        ));
    }
    page.append(&card_list);

    // ---- the one repair worth offering ----
    page.append(&section_title(
        "If a file dialog or a share does nothing",
        "The portal is a background service. One started before the session, or left over from a previous one, answers requests with silence — which looks exactly like the application being broken.",
    ));
    let repair = card();
    let restart = gtk::Button::with_label("Restart the desktop portal");
    restart.add_css_class("pill");
    restart.set_halign(gtk::Align::Start);
    restart.connect_clicked(glib::clone!(
        #[strong]
        app,
        move |_| match share::restart_portal() {
            Ok(()) => {
                app.toast("Desktop portal restarted");
                app.refresh();
            }
            Err(error) => app.toast(&error),
        }
    ));
    repair.append(&restart);
    let note = gtk::Label::new(Some(
        "Safe to do at any time. Anything mid-way through a file dialog will need to be asked for again.",
    ));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("note");
    repair.append(&note);
    page.append(&repair);
    page_scroll(&page)
}

// ---- running the fixes ---------------------------------------------------

/// What a running task reports. One vocabulary for every kind of work, so
/// the dialog below does not have to know whether it is watching a package
/// install, a module build or a video export.
enum TaskEvent {
    Stage(String),
    Progress { label: String, fraction: f64 },
    Log(String),
    Failed(String),
    Finished { ok: bool },
}

struct TaskView {
    dialog: adw::Dialog,
    stage: gtk::Label,
    spinner: adw::Spinner,
    icon: gtk::Image,
    progress: gtk::ProgressBar,
    summary: gtk::Label,
    log: gtk::TextBuffer,
    log_view: gtk::TextView,
    close: gtk::Button,
    failure: RefCell<Option<String>>,
}

impl TaskView {
    fn append_log(&self, line: &str) {
        let mut end = self.log.end_iter();
        self.log.insert(&mut end, line);
        self.log.insert(&mut end, "\n");
        let mark = self.log.create_mark(None, &self.log.end_iter(), false);
        self.log_view.scroll_mark_onscreen(&mark);
    }

    fn handle(&self, event: TaskEvent) -> Option<bool> {
        match event {
            TaskEvent::Stage(text) => {
                self.stage.set_text(&text);
                self.append_log(&format!("▸ {text}"));
            }
            TaskEvent::Progress { label, fraction } => {
                self.progress.set_visible(true);
                self.progress.set_fraction(fraction.clamp(0.0, 1.0));
                self.progress.set_text(Some(&label));
            }
            TaskEvent::Log(line) => self.append_log(&line),
            TaskEvent::Failed(message) => {
                self.append_log(&format!("✖ {message}"));
                *self.failure.borrow_mut() = Some(message);
            }
            TaskEvent::Finished { ok } => {
                self.spinner.set_visible(false);
                self.icon.set_visible(true);
                self.progress.set_visible(false);
                self.close.set_sensitive(true);
                self.dialog.set_can_close(true);
                let failure = self.failure.borrow().clone();
                let ok = ok && failure.is_none();
                if ok {
                    self.icon.set_icon_name(Some("emblem-ok-symbolic"));
                    self.icon.add_css_class("success");
                    self.stage.set_text("Done");
                } else {
                    self.icon.set_icon_name(Some("dialog-warning-symbolic"));
                    self.icon.add_css_class("warning");
                    self.stage.set_text("Did not finish");
                    self.summary.set_visible(true);
                    self.summary.set_text(
                        failure
                            .as_deref()
                            .unwrap_or("Something did not complete. The details below say what."),
                    );
                }
                return Some(ok);
            }
        }
        None
    }
}

fn build_task_view(app: &Rc<App>, title: &str) -> Rc<TaskView> {
    let dialog = adw::Dialog::new();
    dialog.set_title(title);
    dialog.set_content_width(620);
    dialog.set_can_close(false);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_start(20);
    body.set_margin_end(20);
    body.set_margin_top(4);
    body.set_margin_bottom(20);
    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let spinner = adw::Spinner::new();
    spinner.set_size_request(22, 22);
    row.append(&spinner);
    let icon = gtk::Image::new();
    icon.set_pixel_size(22);
    icon.set_visible(false);
    row.append(&icon);
    let stage = gtk::Label::new(Some("Starting…"));
    stage.set_xalign(0.0);
    stage.set_hexpand(true);
    stage.set_ellipsize(gtk::pango::EllipsizeMode::End);
    stage.add_css_class("row-title");
    row.append(&stage);
    body.append(&row);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    body.append(&progress);

    let summary = gtk::Label::new(None);
    summary.add_css_class("dim-label");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.set_visible(false);
    body.append(&summary);

    let log = gtk::TextBuffer::new(None);
    let log_view = gtk::TextView::with_buffer(&log);
    log_view.set_editable(false);
    log_view.set_cursor_visible(false);
    log_view.set_monospace(true);
    log_view.set_wrap_mode(gtk::WrapMode::WordChar);
    log_view.add_css_class("task-log");
    log_view.set_left_margin(6);
    log_view.set_right_margin(6);
    let scroller = gtk::ScrolledWindow::builder()
        .child(&log_view)
        .min_content_height(200)
        .vexpand(true)
        .build();
    let expander = gtk::Expander::builder()
        .label("Details")
        .child(&scroller)
        .build();
    body.append(&expander);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label("Close");
    close.add_css_class("pill");
    close.set_sensitive(false);
    close.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.set_can_close(true);
            dialog.close();
        }
    ));
    buttons.append(&close);
    body.append(&buttons);

    let view = Rc::new(TaskView {
        dialog,
        stage,
        spinner,
        icon,
        progress,
        summary,
        log,
        log_view,
        close,
        failure: RefCell::new(None),
    });
    view.dialog.present(Some(&app.window));
    view
}

/// Drains a task's events on the main loop until it finishes, then
/// re-reads the system so every page reflects what just happened.
fn poll_task(app: Rc<App>, view: Rc<TaskView>, receiver: Receiver<TaskEvent>, done_toast: String) {
    app.busy.set(true);
    glib::timeout_add_local(Duration::from_millis(80), move || {
        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    if let Some(ok) = view.handle(event) {
                        app.busy.set(false);
                        app.refresh();
                        if ok && !done_toast.is_empty() {
                            app.toast(&done_toast);
                        }
                        return glib::ControlFlow::Break;
                    }
                }
                Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    // The worker went away without a Finished event, which
                    // is a bug rather than a normal end; say so instead of
                    // leaving a spinner turning forever.
                    view.handle(TaskEvent::Failed(
                        "The task ended without reporting a result".into(),
                    ));
                    view.handle(TaskEvent::Finished { ok: false });
                    app.busy.set(false);
                    app.refresh();
                    return glib::ControlFlow::Break;
                }
            }
        }
    });
}

/// Carries out a list of fixes, one after another, in one dialog and with
/// at most one password prompt per privileged step.
fn run_fixes(app: &Rc<App>, fixes: Vec<Fix>) {
    if app.busy.get() {
        app.toast("Something is already running");
        return;
    }
    let packages: Vec<String> = fixes
        .iter()
        .filter_map(|f| match f {
            Fix::Install(names) => Some(names.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    if !packages.is_empty()
        && let Some(reason) = install::unavailable()
    {
        let dialog = adw::AlertDialog::new(Some("Packages cannot be installed"), Some(&reason));
        dialog.add_response("ok", "OK");
        if drivers::which("rvn").is_some() {
            dialog.add_response("terminal", "Open a terminal");
            dialog.set_response_appearance("terminal", adw::ResponseAppearance::Suggested);
        }
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("ok");
        dialog.connect_response(
            None,
            glib::clone!(
                #[strong]
                app,
                #[strong]
                packages,
                move |_, response| {
                    if response == "terminal"
                        && let Err(error) = install::open_in_terminal(&packages)
                    {
                        app.toast(&error);
                    }
                }
            ),
        );
        dialog.present(Some(&app.window));
        return;
    }

    let title = match fixes.as_slice() {
        [Fix::Install(names)] if names.len() == 1 => format!("Installing {}", names[0]),
        [Fix::Install(_)] => "Installing packages".to_string(),
        [Fix::BuildModules(_)] => "Building driver modules".to_string(),
        [Fix::ApplyTweaks] => "Applying system settings".to_string(),
        _ => "Setting this computer up for games".to_string(),
    };
    // One prompt for the whole run rather than one per step: a person who
    // pressed "Fix what is missing" has already said yes to the list.
    let needs_root = fixes
        .iter()
        .any(|fix| matches!(fix, Fix::BuildModules(_) | Fix::ApplyTweaks));
    let start = {
        let app = app.clone();
        let title = title.clone();
        move |secret: Option<tune::Secret>| {
            let view = build_task_view(&app, &title);
            let (sender, receiver) = mpsc::channel();
            let fixes = fixes.clone();
            std::thread::spawn(move || {
                let mut ok = true;
                for fix in fixes {
                    if !run_one_fix(&fix, &sender, secret.as_ref()) {
                        ok = false;
                        break;
                    }
                }
                let _ = sender.send(TaskEvent::Finished { ok });
            });
            poll_task(
                app.clone(),
                view,
                receiver,
                "Done — everything was rechecked".into(),
            );
        }
    };
    if needs_root {
        with_root_password(app, &title, start);
    } else {
        start(None);
    }
}

/// One fix, on the worker thread. `false` stops the rest: a driver that
/// failed to install must not be followed by an attempt to build its
/// modules.
fn run_one_fix(fix: &Fix, sender: &mpsc::Sender<TaskEvent>, secret: Option<&tune::Secret>) -> bool {
    match fix {
        Fix::Install(packages) => {
            let _ = sender.send(TaskEvent::Stage(format!(
                "Installing {}",
                packages.join(", ")
            )));
            let events = match install::install(packages) {
                Ok(events) => events,
                Err(error) => {
                    let _ = sender.send(TaskEvent::Failed(error));
                    return false;
                }
            };
            let mut ok = true;
            for event in events {
                match event {
                    install::Event::Stage(text) => {
                        let _ = sender.send(TaskEvent::Stage(text));
                    }
                    install::Event::Progress { label, done, total } => {
                        let fraction = if total == 0 {
                            0.0
                        } else {
                            done as f64 / total as f64
                        };
                        let _ = sender.send(TaskEvent::Progress {
                            label: format!(
                                "{label} — {} of {}",
                                tune::human_bytes(done),
                                tune::human_bytes(total)
                            ),
                            fraction,
                        });
                    }
                    install::Event::Message(text) => {
                        let _ = sender.send(TaskEvent::Log(text));
                    }
                    install::Event::Failed(message) => {
                        let _ = sender.send(TaskEvent::Failed(message));
                        ok = false;
                    }
                    install::Event::Exited { success } => ok = ok && success,
                }
            }
            if !ok {
                let _ = sender.send(TaskEvent::Log("rvn did not finish successfully".into()));
            }
            ok
        }
        Fix::BuildModules(kernels) => {
            if let Some(how) = tune::Escalation::detect().describes_prompt() {
                let _ = sender.send(TaskEvent::Log(how.to_string()));
            }
            for kernel in kernels {
                let _ = sender.send(TaskEvent::Stage(format!(
                    "Building driver modules for {kernel} — this takes a few minutes"
                )));
                let action = tune::Action::DkmsInstall {
                    kernel: kernel.clone(),
                };
                if let Err(error) = tune::run_as_root(&action, secret) {
                    let _ = sender.send(TaskEvent::Failed(error.message()));
                    return false;
                }
                let _ = sender.send(TaskEvent::Log(format!("✔ modules built for {kernel}")));
            }
            true
        }
        Fix::ApplyTweaks => {
            let _ = sender.send(TaskEvent::Stage("Applying system settings".into()));
            if let Some(how) = tune::Escalation::detect().describes_prompt() {
                let _ = sender.send(TaskEvent::Log(how.to_string()));
            }
            match tune::run_as_root(&tune::Action::ApplyTweaks, secret) {
                Ok(()) => {
                    let _ = sender.send(TaskEvent::Log(
                        "✔ settings written, and saved so they survive a reboot".into(),
                    ));
                    let _ = sender.send(TaskEvent::Log(
                        "The open-file limit takes effect at your next login.".into(),
                    ));
                    true
                }
                Err(error) => {
                    let _ = sender.send(TaskEvent::Failed(error.message()));
                    false
                }
            }
        }
        Fix::Manual(advice) => {
            let _ = sender.send(TaskEvent::Log((*advice).to_string()));
            true
        }
    }
}

/// A privileged action with no dialog: quick, and its own failure is a
/// toast rather than a log. Used for the clock policy and presets, where a
/// progress dialog for a single sysfs write would be absurd.
fn run_root_actions(app: &Rc<App>, what: &str, actions: Vec<tune::Action>) {
    if app.busy.get() {
        app.toast("Something is already running");
        return;
    }
    // Nothing to change means nothing to authorise, so a switch already in
    // the position it was clicked into never asks for anything.
    if actions.iter().all(tune::Action::is_already_done) {
        app.toast(what);
        app.refresh();
        return;
    }
    let what = what.to_string();
    with_root_password(
        app,
        &what,
        glib::clone!(
            #[strong]
            app,
            #[strong]
            what,
            #[strong]
            actions,
            move |secret| start_root_actions(&app, &what, actions.clone(), secret)
        ),
    );
}

fn start_root_actions(
    app: &Rc<App>,
    what: &str,
    actions: Vec<tune::Action>,
    secret: Option<tune::Secret>,
) {
    app.busy.set(true);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut failure = None;
        for action in actions {
            if let Err(error) = tune::run_as_root(&action, secret.as_ref()) {
                failure = Some(error.message());
                break;
            }
        }
        let _ = sender.send(failure);
    });
    let what = what.to_string();
    glib::timeout_add_local(
        Duration::from_millis(80),
        glib::clone!(
            #[strong]
            app,
            move || match receiver.try_recv() {
                Ok(failure) => {
                    app.busy.set(false);
                    match failure {
                        Some(error) => app.toast(&error),
                        None => app.toast(&what),
                    }
                    app.refresh();
                    glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    app.busy.set(false);
                    glib::ControlFlow::Break
                }
            }
        ),
    );
}

/// Exports one recording, with the same dialog the fixes use.
fn run_export(app: &Rc<App>, source: std::path::PathBuf, destination: std::path::PathBuf) {
    if app.busy.get() {
        app.toast("Something is already running");
        return;
    }
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let view = build_task_view(app, &format!("Exporting {name}"));
    let (sender, receiver) = mpsc::channel();
    let total = capture::probe(&source).and_then(|r| r.duration);
    std::thread::spawn(move || {
        let _ = sender.send(TaskEvent::Stage("Encoding".into()));
        let events = match capture::export(&source, &destination) {
            Ok(events) => events,
            Err(error) => {
                let _ = sender.send(TaskEvent::Failed(error));
                let _ = sender.send(TaskEvent::Finished { ok: false });
                return;
            }
        };
        let mut ok = true;
        for event in events {
            match event {
                capture::ExportEvent::Progress { frames, seconds } => {
                    // The exporter reports seconds of video written; the
                    // recording's own length is the total, when it had one.
                    let fraction = total
                        .map(|t| seconds / t.as_secs_f64().max(0.001))
                        .unwrap_or(0.0);
                    let _ = sender.send(TaskEvent::Progress {
                        label: format!("{frames} frames · {seconds:.1} s of video"),
                        fraction,
                    });
                }
                capture::ExportEvent::Log(line) => {
                    let _ = sender.send(TaskEvent::Log(line));
                }
                capture::ExportEvent::Finished {
                    ok: success,
                    message,
                } => {
                    ok = success;
                    if success {
                        let _ = sender
                            .send(TaskEvent::Log(format!("✔ wrote {}", destination.display())));
                    } else {
                        let _ = sender.send(TaskEvent::Failed(message));
                    }
                }
            }
        }
        let _ = sender.send(TaskEvent::Finished { ok });
    });
    poll_task(app.clone(), view, receiver, "Exported".into());
}

// ---- asking for a password -----------------------------------------------

/// Asks for the account password, and hands it back once.
///
/// In this window rather than in a terminal, because a program that sends
/// people to a terminal to type their password is teaching them to type it
/// wherever they are told to. The dialog says what it is for, and the
/// answer goes straight to sudo's standard input — never into an argument,
/// an environment variable, or the log this app prints.
fn ask_for_password(app: &Rc<App>, what: &str, on_answer: impl Fn(Option<tune::Secret>) + 'static) {
    ask_for_password_again(app, what, None, Rc::new(on_answer));
}

/// The same dialog, with room for "that one was wrong" above it.
fn ask_for_password_again(
    app: &Rc<App>,
    what: &str,
    complaint: Option<&str>,
    on_answer: Rc<dyn Fn(Option<tune::Secret>)>,
) {
    let explanation = format!(
        "{what} needs to be done as root. Raven Gaming passes what you type straight to sudo and keeps no copy of it."
    );
    let body = match complaint {
        Some(problem) => format!("{problem}\n\n{explanation}"),
        None => explanation,
    };
    let dialog = adw::AlertDialog::new(Some("Administrator password"), Some(&body));
    let entry = gtk::PasswordEntry::builder()
        .placeholder_text("Password")
        .show_peek_icon(true)
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("ok", "Authorise");
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    let entry_for_response = entry.clone();
    let what = what.to_string();
    // How many goes before giving up, so a stuck loop cannot keep putting
    // the same dialog back.
    let attempts = Rc::new(Cell::new(3u8));
    dialog.connect_response(
        None,
        glib::clone!(
            #[strong]
            app,
            move |_, response| {
                if response != "ok" {
                    on_answer(None);
                    return;
                }
                let typed = entry_for_response.text().to_string();
                // Blanked in the widget as well as taken out of it: a
                // dialog that is dismissed rather than destroyed keeps
                // its text otherwise.
                entry_for_response.set_text("");
                let secret = tune::Secret::new(typed);
                // Checked here, against a command that does nothing, so a
                // typo is answered in this dialog rather than as a job
                // that failed several minutes into a module build.
                if tune::password_is_accepted(&secret) {
                    on_answer(Some(secret));
                    return;
                }
                let left = attempts.get().saturating_sub(1);
                attempts.set(left);
                if left == 0 {
                    app.toast("That password was not accepted");
                    on_answer(None);
                    return;
                }
                ask_for_password_again(
                    &app,
                    &what,
                    Some("That password was not accepted."),
                    on_answer.clone(),
                );
            }
        ),
    );
    dialog.present(Some(&app.window));
    entry.grab_focus();
}

/// Asks for a password first when the machine will need one, then runs
/// `then` with it. Where nothing needs to be typed, `then` runs at once.
fn with_root_password(app: &Rc<App>, what: &str, then: impl Fn(Option<tune::Secret>) + 'static) {
    if tune::Escalation::detect().needs_password() {
        ask_for_password(app, what, then);
    } else {
        then(None);
    }
}

// ---- confirmations -------------------------------------------------------

fn confirm_revert(app: &Rc<App>) {
    let dialog = adw::AlertDialog::new(
        Some("Restore the system's own settings?"),
        Some(
            "The two files Raven Gaming wrote are deleted, so the kernel's defaults come back at the next boot. The values already in force stay until then.\n\nLarge games may stop starting again if the memory-mapping limit was what was keeping them running.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("revert", "Restore defaults");
    dialog.set_response_appearance("revert", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        glib::clone!(
            #[strong]
            app,
            move |_, response| {
                if response == "revert" {
                    run_root_actions(
                        &app,
                        "System settings restored",
                        vec![tune::Action::RevertTweaks],
                    );
                }
            }
        ),
    );
    dialog.present(Some(&app.window));
}

fn confirm_clear_cache(app: &Rc<App>, path: std::path::PathBuf) {
    // Nothing outside the user's own cache is ever deleted, whatever the
    // caller passed in.
    if !install::is_in_user_cache(&path) {
        app.toast("That folder is not inside your cache, so it will not be touched");
        return;
    }
    let dialog = adw::AlertDialog::new(
        Some("Clear the shader cache?"),
        Some(&format!(
            "{} is deleted. Nothing is lost permanently — games rebuild their shaders — but the next run of each game will stutter while it does.",
            path.display()
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("clear", "Clear");
    dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        glib::clone!(
            #[strong]
            app,
            move |_, response| {
                if response != "clear" {
                    return;
                }
                // Checked again here: the dialog has been open, and the
                // check is cheap next to deleting the wrong directory.
                if !install::is_in_user_cache(&path) {
                    app.toast("That folder is not inside your cache, so it was not touched");
                    return;
                }
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => {
                        app.toast("Shader cache cleared");
                        app.refresh();
                    }
                    Err(error) => app.toast(&format!("Could not clear it: {error}")),
                }
            }
        ),
    );
    dialog.present(Some(&app.window));
}

fn confirm_delete_recording(app: &Rc<App>, path: std::path::PathBuf, name: String) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Delete {name}?")),
        Some("The recording file is removed for good. An MP4 already exported from it is kept."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        glib::clone!(
            #[strong]
            app,
            move |_, response| {
                if response != "delete" {
                    return;
                }
                // Only ever a `.rvr` inside the recordings folder.
                let in_folder = path.parent() == Some(capture::recordings_dir().as_path());
                let is_recording = path.extension().is_some_and(|e| e == "rvr");
                if !in_folder || !is_recording {
                    app.toast("That file is not a recording in the recordings folder");
                    return;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        app.toast("Recording deleted");
                        app.refresh();
                    }
                    Err(error) => app.toast(&format!("Could not delete it: {error}")),
                }
            }
        ),
    );
    dialog.present(Some(&app.window));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fix_summary_reads_as_a_sentence() {
        assert_eq!(fix_summary(&[]), "");
        assert_eq!(fix_summary(&[Fix::Install(vec!["a".into()])]), "1 package");
        assert_eq!(
            fix_summary(&[Fix::Install(vec!["a".into(), "b".into()]), Fix::ApplyTweaks]),
            "2 packages and system settings"
        );
        assert_eq!(
            fix_summary(&[
                Fix::Install(vec!["a".into()]),
                Fix::BuildModules(vec!["6.1".into()]),
                Fix::ApplyTweaks,
            ]),
            "1 package, driver modules for 1 kernel, and system settings"
        );
    }

    #[test]
    fn each_missing_package_is_shown_under_its_own_card() {
        use gpu::Vendor;
        assert!(requirement_belongs_to("lib32-nvidia-utils", Vendor::Nvidia));
        assert!(!requirement_belongs_to("lib32-nvidia-utils", Vendor::Amd));
        assert!(requirement_belongs_to("lib32-vulkan-radeon", Vendor::Amd));
        assert!(requirement_belongs_to("lib32-mesa", Vendor::Amd));
        assert!(requirement_belongs_to("vulkan-intel", Vendor::Intel));
        // The loader belongs to no card in particular, so it is listed in
        // the Overview's libraries check rather than under a card.
        assert!(!requirement_belongs_to("vulkan-icd-loader", Vendor::Nvidia));
        assert!(!requirement_belongs_to("vulkan-icd-loader", Vendor::Amd));
    }

    #[test]
    fn every_page_name_has_a_builder() {
        // `build_page` panics on an unknown name; this catches a page
        // added to PAGES and not to the match.
        for page in &PAGES {
            assert!(
                matches!(
                    page.name,
                    "overview"
                        | "graphics"
                        | "performance"
                        | "audio"
                        | "controllers"
                        | "games"
                        | "emulators"
                        | "tools"
                        | "capture"
                        | "sharing"
                ),
                "{} has no builder",
                page.name
            );
        }
    }
}
