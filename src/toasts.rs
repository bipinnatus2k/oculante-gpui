use std::time::Duration;

use crate::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    TopLeft,
    TopRight,
    #[default]
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastLevel {
    pub fn icon(self) -> &'static str {
        match self {
            ToastLevel::Info => icons::INFO,
            ToastLevel::Success => icons::CHECK,
            ToastLevel::Warning => icons::WARNING_CIRCLE,
            ToastLevel::Error => icons::ERROR_CIRCLE,
        }
    }

    pub fn default_duration(self) -> f32 {
        match self {
            ToastLevel::Info | ToastLevel::Success => 3.0,
            ToastLevel::Warning => 4.5,
            ToastLevel::Error => 6.0,
        }
    }
}

const APPEAR_SECS: f32 = 0.18;
const DISAPPEAR_SECS: f32 = 0.2;

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub level: ToastLevel,
    pub total: Option<f32>,
    pub remaining: Option<f32>,
    /// Animation progress in `[0, 1]`; fades out again once `closing`.
    pub shown: f32,
    pub closing: bool,
}

pub struct ToastHandle<'a> {
    toast: &'a mut Toast,
}

impl ToastHandle<'_> {
    pub fn duration(self, duration: impl Into<Option<Duration>>) -> Self {
        let secs = duration.into().map(|d| d.as_secs_f32().max(0.05));
        self.toast.total = secs;
        self.toast.remaining = secs;
        self
    }
}

pub struct Toasts {
    anchor: Anchor,
    items: Vec<Toast>,
    next_id: u64,
}

impl Default for Toasts {
    fn default() -> Self {
        Self {
            anchor: Anchor::BottomLeft,
            items: Vec::new(),
            next_id: 0,
        }
    }
}

impl Toasts {
    pub fn with_anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    fn push(&mut self, text: impl Into<String>, level: ToastLevel) -> ToastHandle<'_> {
        let id = self.next_id;
        self.next_id += 1;
        let secs = Some(level.default_duration());
        self.items.push(Toast {
            id,
            text: text.into(),
            level,
            total: secs,
            remaining: secs,
            shown: 0.0,
            closing: false,
        });
        ToastHandle {
            toast: self.items.last_mut().expect("just pushed a toast"),
        }
    }

    pub fn info(&mut self, text: impl Into<String>) -> ToastHandle<'_> {
        self.push(text, ToastLevel::Info)
    }

    pub fn success(&mut self, text: impl Into<String>) -> ToastHandle<'_> {
        self.push(text, ToastLevel::Success)
    }

    pub fn warning(&mut self, text: impl Into<String>) -> ToastHandle<'_> {
        self.push(text, ToastLevel::Warning)
    }

    pub fn error(&mut self, text: impl Into<String>) -> ToastHandle<'_> {
        self.push(text, ToastLevel::Error)
    }

    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// Advances every toast's animation by `dt`. The hovered toast's countdown is paused, which
    /// is what kept the egui widget on screen while the pointer rested on it.
    ///
    /// Rendering only repaints on state changes, so a shell must call this on a timer.
    pub fn tick(&mut self, dt: f32, hovered: Option<u64>) -> bool {
        if self.items.is_empty() {
            return false;
        }
        let dt = dt.min(0.1);
        for toast in &mut self.items {
            if toast.closing {
                toast.shown -= dt / DISAPPEAR_SECS;
                continue;
            }
            toast.shown = (toast.shown + dt / APPEAR_SECS).min(1.0);
            let Some(remaining) = toast.remaining.as_mut() else {
                continue;
            };
            if hovered == Some(toast.id) {
                continue;
            }
            *remaining -= dt;
            if *remaining <= 0.0 {
                toast.closing = true;
            }
        }
        self.items.retain(|t| !(t.closing && t.shown <= 0.0));
        true
    }

    /// Dismisses a toast, letting `tick` fade it out.
    pub fn dismiss(&mut self, id: u64) {
        if let Some(toast) = self.items.iter_mut().find(|t| t.id == id) {
            toast.closing = true;
        }
    }
}
