//! Jiggler — a tiny macOS app that keeps your Mac awake and your status "Available"
//! by gently moving the pointer whenever you've been idle for a while.

mod system;

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAboutPanelOptionCredits, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSEventModifierFlags, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSImage, NSImageSymbolConfiguration, NSImageView,
    NSLayoutAttribute, NSLinkAttributeName, NSMenu, NSMenuItem, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSPopUpButton, NSStackView, NSStackViewGravity, NSSwitch,
    NSTextAlignment, NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
    NSWindow, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSDictionary, NSEdgeInsets,
    NSMutableAttributedString, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSURL, NSUserDefaults, ns_string,
};

use crate::system::SleepAssertion;

const AUTHOR: &str = "Adil";
const REPO_URL: &str = "https://github.com/adilkhali/jiggler";
const LICENSE_NAME: &str = "PolyForm Noncommercial License 1.0.0";
const LICENSE_URL: &str = "https://polyformproject.org/licenses/noncommercial/1.0.0";

const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// Idle durations offered in the UI: (seconds, label).
const INTERVALS: [(u64, &str); 5] = [
    (15, "15 seconds"),
    (30, "30 seconds"),
    (60, "1 minute"),
    (120, "2 minutes"),
    (240, "4 minutes"),
];
const DEFAULT_INTERVAL_SECS: u64 = 30;

const JIGGLE_RADIUS: f64 = 20.0;
const JIGGLE_LOOPS: u32 = 2;
/// Input more recent than this means the user is at the keyboard right now.
const USER_ACTIVE_SECS: f64 = 2.0;

const DEFAULTS_ENABLED: &str = "enabled";
const DEFAULTS_INTERVAL: &str = "intervalSeconds";

const CONTENT_WIDTH: f64 = 360.0;
const PADDING: f64 = 20.0;
const ICON_SIZE: f64 = 56.0;
const CARD_PADDING: f64 = 12.0;

#[derive(Default)]
struct Ivars {
    window: OnceCell<Retained<NSWindow>>,
    toggle: OnceCell<Retained<NSSwitch>>,
    interval_popup: OnceCell<Retained<NSPopUpButton>>,
    status_title: OnceCell<Retained<NSTextField>>,
    status_detail: OnceCell<Retained<NSTextField>>,
    permission_card: OnceCell<Retained<NSBox>>,
    assertion: RefCell<Option<SleepAssertion>>,
    trusted: Cell<bool>,
    last_jiggle: Cell<Option<Instant>>,
    jiggle_count: Cell<u64>,
    jiggling: Arc<AtomicBool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "JigglerAppDelegate"]
    #[ivars = Ivars]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.launch();
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            self.ivars().assertion.borrow_mut().take();
        }
    }

    impl AppDelegate {
        #[unsafe(method(toggleChanged:))]
        fn toggle_changed(&self, _sender: Option<&AnyObject>) {
            let enabled = self.is_enabled();
            NSUserDefaults::standardUserDefaults()
                .setBool_forKey(enabled, &NSString::from_str(DEFAULTS_ENABLED));
            self.apply_enabled(enabled);
        }

        #[unsafe(method(intervalChanged:))]
        fn interval_changed(&self, _sender: Option<&AnyObject>) {
            NSUserDefaults::standardUserDefaults().setInteger_forKey(
                self.interval_secs() as isize,
                &NSString::from_str(DEFAULTS_INTERVAL),
            );
            self.refresh_status();
        }

        #[unsafe(method(openAccessibilitySettings:))]
        fn open_accessibility_settings(&self, _sender: Option<&AnyObject>) {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(ACCESSIBILITY_SETTINGS_URL)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        }

        #[unsafe(method(showAbout:))]
        fn show_about(&self, _sender: Option<&AnyObject>) {
            let credits = about_credits();
            let credits: &AnyObject = &credits;
            let key = unsafe { NSAboutPanelOptionCredits };
            let options = NSDictionary::from_slices(&[key], &[credits]);
            unsafe {
                NSApplication::sharedApplication(self.mtm())
                    .orderFrontStandardAboutPanelWithOptions(&options)
            };
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.on_tick();
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars::default());
        unsafe { msg_send![super(this), init] }
    }

    fn launch(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        app.setMainMenu(Some(&build_main_menu(mtm, &app)));

        let window = self.build_window(mtm);
        let trusted = system::accessibility_trusted();
        self.ivars().trusted.set(trusted);

        let enabled = load_enabled();
        let toggle = self.ivars().toggle.get().expect("toggle built");
        toggle.setState(if enabled {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.apply_enabled(enabled);
        self.refresh_permission_ui();

        window.center();
        window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);

        // Common modes keep the timer firing while menus or sheets are open.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                1.0,
                self,
                sel!(tick:),
                None,
                true,
            )
        };
        timer.setTolerance(0.2);
        unsafe { NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };

        if !trusted {
            system::request_accessibility();
        }
    }

    fn on_tick(&self) {
        let ivars = self.ivars();
        let trusted = system::accessibility_trusted();
        if trusted != ivars.trusted.get() {
            ivars.trusted.set(trusted);
            self.refresh_permission_ui();
        }

        if self.is_enabled() && trusted && !ivars.jiggling.load(Ordering::Acquire) {
            let interval = self.interval_secs() as f64;
            if system::idle_seconds() >= interval && self.secs_since_jiggle() >= interval {
                self.start_jiggle();
            }
        }
        self.refresh_status();
    }

    fn start_jiggle(&self) {
        let ivars = self.ivars();
        let busy = Arc::clone(&ivars.jiggling);
        busy.store(true, Ordering::Release);
        ivars.last_jiggle.set(Some(Instant::now()));

        let spawned = std::thread::Builder::new().name("jiggle".into()).spawn({
            let busy = Arc::clone(&busy);
            move || {
                system::jiggle(JIGGLE_RADIUS, JIGGLE_LOOPS);
                busy.store(false, Ordering::Release);
            }
        });
        match spawned {
            Ok(_) => ivars.jiggle_count.set(ivars.jiggle_count.get() + 1),
            Err(_) => busy.store(false, Ordering::Release),
        }
    }

    fn apply_enabled(&self, enabled: bool) {
        let mut assertion = self.ivars().assertion.borrow_mut();
        if enabled {
            if assertion.is_none() {
                *assertion = SleepAssertion::acquire("Jiggler is keeping your Mac awake");
            }
        } else {
            assertion.take();
        }
        drop(assertion);
        self.refresh_status();
    }

    fn is_enabled(&self) -> bool {
        self.ivars()
            .toggle
            .get()
            .is_some_and(|t| t.state() == NSControlStateValueOn)
    }

    fn interval_secs(&self) -> u64 {
        let index = self
            .ivars()
            .interval_popup
            .get()
            .map_or(-1, |p| p.indexOfSelectedItem());
        usize::try_from(index)
            .ok()
            .and_then(|i| INTERVALS.get(i))
            .map_or(DEFAULT_INTERVAL_SECS, |&(secs, _)| secs)
    }

    fn secs_since_jiggle(&self) -> f64 {
        self.ivars()
            .last_jiggle
            .get()
            .map_or(f64::INFINITY, |t| t.elapsed().as_secs_f64())
    }

    fn refresh_permission_ui(&self) {
        let ivars = self.ivars();
        if let Some(card) = ivars.permission_card.get() {
            card.setHidden(ivars.trusted.get());
        }
        self.fit_window();
        self.refresh_status();
    }

    fn refresh_status(&self) {
        let ivars = self.ivars();
        let (Some(title), Some(detail)) = (ivars.status_title.get(), ivars.status_detail.get())
        else {
            return;
        };

        let (heading, color, text) = if !self.is_enabled() {
            (
                "Off",
                NSColor::secondaryLabelColor(),
                "Your Mac will sleep and your status will go idle as usual.".to_string(),
            )
        } else if !ivars.trusted.get() {
            ("On — limited", NSColor::systemOrangeColor(), "Keeping your Mac awake. Allow Accessibility access so Jiggler can also move the pointer.".to_string())
        } else if ivars.jiggling.load(Ordering::Acquire) {
            (
                "On",
                NSColor::systemGreenColor(),
                "Moving the pointer…".to_string(),
            )
        } else {
            let interval = self.interval_secs() as f64;
            let idle = system::idle_seconds();
            let moved = match ivars.jiggle_count.get() {
                0 => String::new(),
                1 => " Moved once so far.".to_string(),
                n => format!(" Moved {n} times so far."),
            };
            if idle < USER_ACTIVE_SECS {
                (
                    "On",
                    NSColor::systemGreenColor(),
                    format!("You're active, so no need to move the pointer.{moved}"),
                )
            } else {
                let remaining = (interval - idle)
                    .max(interval - self.secs_since_jiggle())
                    .max(0.0);
                (
                    "On",
                    NSColor::systemGreenColor(),
                    format!(
                        "Next pointer move in {}.{moved}",
                        format_duration((remaining.ceil() as u64).max(1))
                    ),
                )
            }
        };

        title.setStringValue(&NSString::from_str(&format!("● {heading}")));
        title.setTextColor(Some(&color));
        detail.setStringValue(&NSString::from_str(&text));
    }

    /// Resizes the window to its content, keeping the top edge in place.
    fn fit_window(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let Some(content) = window.contentView() else {
            return;
        };
        content.layoutSubtreeIfNeeded();
        let size = content.fittingSize();
        let old = window.frame();
        let new = window.frameRectForContentRect(NSRect::new(NSPoint::ZERO, size));
        let origin = NSPoint::new(
            old.origin.x,
            old.origin.y + old.size.height - new.size.height,
        );
        window.setFrame_display_animate(NSRect::new(origin, new.size), true, window.isVisible());
    }

    fn build_window(&self, mtm: MainThreadMarker) -> Retained<NSWindow> {
        let ivars = self.ivars();
        let target: &AnyObject = self.as_ref();

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::ZERO,
                    NSSize::new(CONTENT_WIDTH + 2.0 * PADDING, 300.0),
                ),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("Jiggler"));

        // Header: app icon + name + one-line explanation.
        let icon = NSImageView::new(mtm);
        icon.setImage(
            NSApplication::sharedApplication(mtm)
                .applicationIconImage()
                .as_deref(),
        );
        set_size(&icon, ICON_SIZE, ICON_SIZE);
        let name = label("Jiggler", &NSFont::boldSystemFontOfSize(18.0), None, mtm);
        let tagline = wrapping_label(
            "Keeps your Mac awake and your status Available by gently moving the pointer while you're away.",
            &NSFont::systemFontOfSize(12.0),
            Some(&NSColor::secondaryLabelColor()),
            CONTENT_WIDTH - ICON_SIZE - 14.0,
            mtm,
        );
        let header_text = stack(
            &[&name, &tagline],
            NSUserInterfaceLayoutOrientation::Vertical,
            3.0,
            mtm,
        );
        let header = stack(
            &[&icon, &header_text],
            NSUserInterfaceLayoutOrientation::Horizontal,
            14.0,
            mtm,
        );

        let card = self.build_permission_card(target, mtm);

        // Controls.
        let toggle = NSSwitch::new(mtm);
        unsafe {
            toggle.setTarget(Some(target));
            toggle.setAction(Some(sel!(toggleChanged:)));
        }
        let toggle_row = row(
            &label("Keep awake", &NSFont::systemFontOfSize(13.0), None, mtm),
            &toggle,
            mtm,
        );

        let popup =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        let titles: Vec<Retained<NSString>> = INTERVALS
            .iter()
            .map(|(_, t)| NSString::from_str(t))
            .collect();
        popup.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
        popup.selectItemAtIndex(load_interval_index() as isize);
        unsafe {
            popup.setTarget(Some(target));
            popup.setAction(Some(sel!(intervalChanged:)));
        }
        let interval_row = row(
            &label(
                "Move pointer after idle for",
                &NSFont::systemFontOfSize(13.0),
                None,
                mtm,
            ),
            &popup,
            mtm,
        );

        // Status.
        let status_title = label("", &NSFont::boldSystemFontOfSize(13.0), None, mtm);
        let status_detail = wrapping_label(
            "",
            &NSFont::systemFontOfSize(12.0),
            Some(&NSColor::secondaryLabelColor()),
            CONTENT_WIDTH,
            mtm,
        );
        let status = stack(
            &[&status_title, &status_detail],
            NSUserInterfaceLayoutOrientation::Vertical,
            3.0,
            mtm,
        );

        let separator_top = separator(mtm);
        let separator_bottom = separator(mtm);
        let root = stack(
            &[
                &header,
                &card,
                &separator_top,
                &toggle_row,
                &interval_row,
                &separator_bottom,
                &status,
            ],
            NSUserInterfaceLayoutOrientation::Vertical,
            14.0,
            mtm,
        );
        root.setEdgeInsets(NSEdgeInsets {
            top: PADDING,
            left: PADDING,
            bottom: PADDING,
            right: PADDING,
        });
        let full_width: [&NSView; 7] = [
            &header,
            &card,
            &separator_top,
            &toggle_row,
            &interval_row,
            &separator_bottom,
            &status,
        ];
        for view in full_width {
            view.widthAnchor()
                .constraintEqualToConstant(CONTENT_WIDTH)
                .setActive(true);
        }
        window.setContentView(Some(&root));

        let _ = ivars.window.set(window.clone());
        let _ = ivars.toggle.set(toggle);
        let _ = ivars.interval_popup.set(popup);
        let _ = ivars.status_title.set(status_title);
        let _ = ivars.status_detail.set(status_detail);
        let _ = ivars.permission_card.set(card);
        window
    }

    fn build_permission_card(&self, target: &AnyObject, mtm: MainThreadMarker) -> Retained<NSBox> {
        let orange = NSColor::systemOrangeColor();
        let card = NSBox::new(mtm);
        card.setBoxType(NSBoxType::Custom);
        card.setTitlePosition(NSTitlePosition::NoTitle);
        card.setCornerRadius(10.0);
        card.setBorderWidth(1.0);
        card.setBorderColor(&orange.colorWithAlphaComponent(0.45));
        card.setFillColor(&orange.colorWithAlphaComponent(0.10));
        card.setContentViewMargins(NSSize::new(0.0, 0.0));

        let icon = NSImageView::new(mtm);
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("hand.raised.fill"),
            Some(ns_string!("Permission needed")),
        ) {
            icon.setImage(Some(&image));
        }
        icon.setSymbolConfiguration(Some(
            &NSImageSymbolConfiguration::configurationWithPointSize_weight(18.0, 0.0),
        ));
        icon.setContentTintColor(Some(&orange));

        let text_width = CONTENT_WIDTH - 2.0 * CARD_PADDING - 24.0 - 10.0;
        let title = label(
            "Permission needed",
            &NSFont::boldSystemFontOfSize(13.0),
            None,
            mtm,
        );
        let body = wrapping_label(
            "To move the pointer, Jiggler needs Accessibility access. Open System Settings and turn on Jiggler.\n\nAlready on but still not working? Select Jiggler, remove it with “–”, then add it again with “+”.",
            &NSFont::systemFontOfSize(12.0),
            Some(&NSColor::secondaryLabelColor()),
            text_width,
            mtm,
        );
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Open System Settings…"),
                Some(target),
                Some(sel!(openAccessibilitySettings:)),
                mtm,
            )
        };
        button.setKeyEquivalent(ns_string!("\r"));

        let text = stack(
            &[&title, &body, &button],
            NSUserInterfaceLayoutOrientation::Vertical,
            8.0,
            mtm,
        );
        let content = stack(
            &[&icon, &text],
            NSUserInterfaceLayoutOrientation::Horizontal,
            10.0,
            mtm,
        );
        content.setAlignment(NSLayoutAttribute::Top);
        content.setEdgeInsets(NSEdgeInsets {
            top: CARD_PADDING,
            left: CARD_PADDING,
            bottom: CARD_PADDING,
            right: CARD_PADDING,
        });
        set_size(&icon, 24.0, 22.0);

        if let Some(container) = card.contentView() {
            content.setTranslatesAutoresizingMaskIntoConstraints(false);
            container.addSubview(&content);
            content
                .leadingAnchor()
                .constraintEqualToAnchor(&container.leadingAnchor())
                .setActive(true);
            content
                .trailingAnchor()
                .constraintEqualToAnchor(&container.trailingAnchor())
                .setActive(true);
            content
                .topAnchor()
                .constraintEqualToAnchor(&container.topAnchor())
                .setActive(true);
            content
                .bottomAnchor()
                .constraintEqualToAnchor(&container.bottomAnchor())
                .setActive(true);
        }
        card
    }
}

/// Credits shown in the About panel: author, project link and license.
fn about_credits() -> Retained<NSMutableAttributedString> {
    let credits = NSMutableAttributedString::new();
    let repo_label = REPO_URL.trim_start_matches("https://");
    for (text, link) in [
        (format!("Made by {AUTHOR}\n"), None),
        (repo_label.to_string(), Some(REPO_URL)),
        (
            "\n\nFree for noncommercial use under the\n".to_string(),
            None,
        ),
        (LICENSE_NAME.to_string(), Some(LICENSE_URL)),
    ] {
        credits.appendAttributedString(&credits_run(&text, link));
    }
    credits
}

fn credits_run(text: &str, link: Option<&str>) -> Retained<NSAttributedString> {
    let font = NSFont::systemFontOfSize(NSFont::smallSystemFontSize());
    let color = NSColor::labelColor();
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    let url = link.and_then(|l| NSURL::URLWithString(&NSString::from_str(l)));

    let mut keys: Vec<&NSAttributedStringKey> = unsafe {
        vec![
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let mut values: Vec<&AnyObject> = vec![&font, &color, &paragraph];
    if let Some(url) = &url {
        keys.push(unsafe { NSLinkAttributeName });
        values.push(url);
    }
    let attributes = NSDictionary::from_slices(&keys, &values);
    // SAFETY: every value matches the type its attribute key expects.
    unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(text), &attributes) }
}

fn load_enabled() -> bool {
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(DEFAULTS_ENABLED);
    defaults.objectForKey(&key).is_none() || defaults.boolForKey(&key)
}

fn load_interval_index() -> usize {
    let saved = NSUserDefaults::standardUserDefaults()
        .integerForKey(&NSString::from_str(DEFAULTS_INTERVAL));
    let wanted = u64::try_from(saved).unwrap_or(0);
    INTERVALS
        .iter()
        .position(|&(secs, _)| secs == wanted)
        .or_else(|| {
            INTERVALS
                .iter()
                .position(|&(secs, _)| secs == DEFAULT_INTERVAL_SECS)
        })
        .unwrap_or(0)
}

fn format_duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        _ if secs.is_multiple_of(60) => format!("{}m", secs / 60),
        _ => format!("{}m {:02}s", secs / 60, secs % 60),
    }
}

fn label(
    text: &str,
    font: &NSFont,
    color: Option<&NSColor>,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setFont(Some(font));
    if color.is_some() {
        field.setTextColor(color);
    }
    field
}

fn wrapping_label(
    text: &str,
    font: &NSFont,
    color: Option<&NSColor>,
    width: f64,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let field = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    field.setFont(Some(font));
    field.setTextColor(color);
    field.setSelectable(false);
    field.setPreferredMaxLayoutWidth(width);
    field
        .widthAnchor()
        .constraintEqualToConstant(width)
        .setActive(true);
    field
}

fn stack(
    views: &[&NSView],
    orientation: NSUserInterfaceLayoutOrientation,
    spacing: f64,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(views), mtm);
    stack.setOrientation(orientation);
    stack.setSpacing(spacing);
    stack.setAlignment(
        if orientation == NSUserInterfaceLayoutOrientation::Vertical {
            NSLayoutAttribute::Leading
        } else {
            NSLayoutAttribute::CenterY
        },
    );
    stack
}

/// A full-width row with a label on the left and a control on the right.
fn row(label: &NSView, control: &NSView, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.addView_inGravity(label, NSStackViewGravity::Leading);
    row.addView_inGravity(control, NSStackViewGravity::Trailing);
    row
}

fn separator(mtm: MainThreadMarker) -> Retained<NSBox> {
    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);
    line
}

fn set_size(view: &NSView, width: f64, height: f64) {
    view.widthAnchor()
        .constraintEqualToConstant(width)
        .setActive(true);
    view.heightAnchor()
        .constraintEqualToConstant(height)
        .setActive(true);
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

fn build_main_menu(mtm: MainThreadMarker, app: &NSApplication) -> Retained<NSMenu> {
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&menu_item(mtm, "About Jiggler", Some(sel!(showAbout:)), ""));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&menu_item(mtm, "Hide Jiggler", Some(sel!(hide:)), "h"));
    let hide_others = menu_item(mtm, "Hide Others", Some(sel!(hideOtherApplications:)), "h");
    hide_others
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Option);
    app_menu.addItem(&hide_others);
    app_menu.addItem(&menu_item(
        mtm,
        "Show All",
        Some(sel!(unhideAllApplications:)),
        "",
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&menu_item(mtm, "Quit Jiggler", Some(sel!(terminate:)), "q"));

    let window_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Window"));
    window_menu.addItem(&menu_item(
        mtm,
        "Minimize",
        Some(sel!(performMiniaturize:)),
        "m",
    ));
    window_menu.addItem(&menu_item(mtm, "Close", Some(sel!(performClose:)), "w"));
    app.setWindowsMenu(Some(&window_menu));

    let main_menu = NSMenu::new(mtm);
    for submenu in [&app_menu, &window_menu] {
        let item = NSMenuItem::new(mtm);
        item.setSubmenu(Some(submenu));
        main_menu.addItem(&item);
    }
    main_menu
}

fn main() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
