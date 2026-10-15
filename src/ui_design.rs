//! Native equivalents of the 17 tokens and finite motion matrix in AGENTS.md.
//! Colors are Win32 COLORREF; alpha is kept separate until raster/composition.

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}
pub const BG_CANVAS: u32 = rgb(9, 9, 11);
pub const BG_SURFACE: u32 = rgb(18, 18, 21);
pub const BG_ELEVATED: u32 = rgb(28, 28, 33);
pub const BG_GLASS: (u32, f32) = (BG_ELEVATED, 0.78);
pub const BORDER_SUBTLE: (u32, f32) = (rgb(255, 255, 255), 0.07);
pub const BORDER_FOCUS: u32 = rgb(59, 130, 246);
pub const SHADOW_INSET_HIGHLIGHT: (u32, f32) = (rgb(255, 255, 255), 0.1);
pub const SHADOW_ELEVATION: Shadow = Shadow {
    x: 0,
    y: 16,
    blur: 36,
    spread: -8,
    color: 0,
    alpha: 0.6,
};
pub const TEXT_PRIMARY: u32 = rgb(244, 244, 246);
pub const TEXT_SECONDARY: u32 = rgb(161, 161, 170);
pub const TEXT_MUTED: u32 = rgb(113, 113, 122);
pub const EASE_OUT_EXPO: Curve = Curve(0.22, 1.0, 0.36, 1.0);
pub const EASE_SHEET: Curve = Curve(0.32, 0.72, 0.0, 1.0);
pub const EASE_IN_OUT: Curve = Curve(0.65, 0.0, 0.35, 1.0);
pub const DURATION_FAST: u32 = 150;
pub const DURATION_NORMAL: u32 = 200;
pub const DURATION_SHEET: u32 = 250;
pub const REDUCED_DURATION_MS: f64 = 0.01;
pub const ICON_DIP: i32 = 20;
pub const SHORTCUT_DIP: i32 = 24;
pub const BLUR_DIP: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub x: i32,
    pub y: i32,
    pub blur: i32,
    pub spread: i32,
    pub color: u32,
    pub alpha: f32,
}

pub fn blend(front: u32, back: u32, alpha: f32) -> u32 {
    let mut result = 0;
    for shift in [0, 8, 16] {
        let f = ((front >> shift) & 255) as f32;
        let b = ((back >> shift) & 255) as f32;
        result |= ((f * alpha + b * (1.0 - alpha)).round() as u32) << shift;
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve(pub f64, pub f64, pub f64, pub f64);

#[derive(Clone, Copy, Debug)]
pub struct Cubic {
    pub offset: f64,
    pub coefficients: [f32; 4],
}

impl Curve {
    fn coordinate(t: f64, a: f64, b: f64) -> f64 {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    }

    fn derivative(t: f64, a: f64, b: f64) -> f64 {
        3.0 * (1.0 - t).powi(2) * a + 6.0 * (1.0 - t) * t * (b - a) + 3.0 * t * t * (1.0 - b)
    }

    pub fn sample(self, time: f64) -> (f64, f64) {
        let time = time.clamp(0.0, 1.0);
        let (mut low, mut high) = (0.0, 1.0);
        // Invert CSS's time coordinate once when scheduling, never per frame.
        for _ in 0..42 {
            let t = (low + high) / 2.0;
            if Self::coordinate(t, self.0, self.2) < time {
                low = t;
            } else {
                high = t;
            }
        }
        let t = if time == 0.0 {
            0.0
        } else if time == 1.0 {
            1.0
        } else {
            (low + high) / 2.0
        };
        (
            Self::coordinate(t, self.1, self.3),
            Self::derivative(t, self.1, self.3) / Self::derivative(t, self.0, self.2).max(1e-9),
        )
    }

    pub fn segments(self, from: f32, to: f32, milliseconds: u32) -> Vec<Cubic> {
        const PIECES: usize = 48;
        let seconds = milliseconds.max(1) as f64 / 1000.0;
        let step = seconds / PIECES as f64;
        let distance = (to - from) as f64;
        (0..PIECES)
            .map(|i| {
                let (a, da) = self.sample(i as f64 / PIECES as f64);
                let (b, db) = self.sample((i + 1) as f64 / PIECES as f64);
                let a = from as f64 + distance * a;
                let b = from as f64 + distance * b;
                let va = distance * da / seconds;
                let vb = distance * db / seconds;
                Cubic {
                    offset: i as f64 * step,
                    coefficients: [
                        a as f32,
                        va as f32,
                        (3.0 * (b - a) / step.powi(2) - (2.0 * va + vb) / step) as f32,
                        (2.0 * (a - b) / step.powi(3) + (va + vb) / step.powi(2)) as f32,
                    ],
                }
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub opacity: f32,
}
impl Pose {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        opacity: 1.0,
    };
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub from: Pose,
    pub to: Pose,
    pub milliseconds: u32,
    pub curve: Curve,
}
impl Motion {
    pub fn backdrop(enter: bool) -> Self {
        Self {
            from: Pose {
                opacity: if enter { 0.0 } else { 1.0 },
                ..Pose::IDENTITY
            },
            to: Pose {
                opacity: if enter { 1.0 } else { 0.0 },
                ..Pose::IDENTITY
            },
            milliseconds: DURATION_NORMAL,
            curve: EASE_OUT_EXPO,
        }
    }
    pub fn dialog(enter: bool) -> Self {
        Self {
            from: if enter {
                Pose {
                    y: 8.0,
                    scale: 0.96,
                    opacity: 0.0,
                    ..Pose::IDENTITY
                }
            } else {
                Pose::IDENTITY
            },
            to: if enter {
                Pose::IDENTITY
            } else {
                Pose {
                    y: 4.0,
                    scale: 0.98,
                    opacity: 0.0,
                    ..Pose::IDENTITY
                }
            },
            milliseconds: if enter {
                DURATION_SHEET - 30
            } else {
                DURATION_FAST
            },
            curve: if enter { EASE_SHEET } else { EASE_OUT_EXPO },
        }
    }
    pub fn toast(enter: bool) -> Self {
        Self {
            from: if enter {
                Pose {
                    y: 16.0,
                    scale: 0.95,
                    opacity: 0.0,
                    ..Pose::IDENTITY
                }
            } else {
                Pose::IDENTITY
            },
            to: if enter {
                Pose::IDENTITY
            } else {
                Pose {
                    y: 8.0,
                    scale: 0.96,
                    opacity: 0.0,
                    ..Pose::IDENTITY
                }
            },
            milliseconds: if enter { DURATION_SHEET - 30 } else { 160 },
            curve: if enter { EASE_SHEET } else { EASE_OUT_EXPO },
        }
    }
    pub fn press(down: bool) -> Self {
        Self {
            from: Pose {
                scale: if down { 1.0 } else { 0.97 },
                ..Pose::IDENTITY
            },
            to: Pose {
                scale: if down { 0.97 } else { 1.0 },
                ..Pose::IDENTITY
            },
            milliseconds: DURATION_FAST,
            curve: EASE_OUT_EXPO,
        }
    }
    pub fn slide(delta: f32) -> Self {
        Self {
            from: Pose {
                x: delta,
                ..Pose::IDENTITY
            },
            to: Pose::IDENTITY,
            milliseconds: DURATION_NORMAL,
            curve: EASE_IN_OUT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Calendar,
    Day,
    Completed,
    Showcase,
}
impl View {
    pub fn index(self) -> usize {
        match self {
            Self::Calendar => 0,
            Self::Day => 1,
            Self::Completed => 2,
            Self::Showcase => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonRole {
    Primary,
    Secondary,
    Ghost,
}

#[derive(Clone, Copy, Debug)]
pub struct PaletteCommand {
    pub id: i32,
    pub category: &'static str,
    pub title: &'static str,
    pub keywords: &'static str,
    pub shortcut: &'static str,
}

pub fn search_commands(commands: &[PaletteCommand], query: &str) -> Vec<usize> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    commands
        .iter()
        .enumerate()
        .filter_map(|(i, command)| {
            let haystack = format!(
                "{} {} {}",
                command.category, command.title, command.keywords
            )
            .to_lowercase();
            words
                .iter()
                .all(|word| haystack.contains(word))
                .then_some(i)
        })
        .collect()
}

pub fn wrap_selection(current: Option<usize>, direction: i32, count: usize) -> Option<usize> {
    if count == 0 {
        None
    } else {
        Some((current.unwrap_or(0) as i64 + direction as i64).rem_euclid(count as i64) as usize)
    }
}

pub fn palette_shortcut(key: u16, control: bool, repeat: bool) -> bool {
    key == b'K' as u16 && control && !repeat
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_preserve_channels_alpha_and_shadow_geometry() {
        assert_eq!(BG_CANVAS, 0x000b0909);
        assert_eq!(BG_SURFACE, rgb(18, 18, 21));
        assert_eq!(BG_GLASS, (BG_ELEVATED, 0.78));
        assert_eq!(BORDER_SUBTLE.1, 0.07);
        assert_eq!(BORDER_FOCUS, rgb(59, 130, 246));
        assert_eq!(SHADOW_INSET_HIGHLIGHT.1, 0.1);
        assert_eq!(
            (
                SHADOW_ELEVATION.x,
                SHADOW_ELEVATION.y,
                SHADOW_ELEVATION.blur,
                SHADOW_ELEVATION.spread,
                SHADOW_ELEVATION.alpha
            ),
            (0, 16, 36, -8, 0.6)
        );
        assert_eq!(
            (TEXT_PRIMARY, TEXT_SECONDARY, TEXT_MUTED),
            (rgb(244, 244, 246), rgb(161, 161, 170), rgb(113, 113, 122))
        );
        assert_eq!(
            (DURATION_FAST, DURATION_NORMAL, DURATION_SHEET),
            (150, 200, 250)
        );
        assert_eq!((ICON_DIP, SHORTCUT_DIP, BLUR_DIP), (20, 24, 12));
        assert_eq!(REDUCED_DURATION_MS, 0.01);
        assert_eq!(blend(rgb(255, 255, 255), 0, 0.1), rgb(26, 26, 26));
    }

    #[test]
    fn finite_cubic_segments_match_css_time_inversion() {
        for curve in [EASE_OUT_EXPO, EASE_SHEET, EASE_IN_OUT] {
            let segments = curve.segments(0.0, 1.0, 220);
            let step = 0.22 / segments.len() as f64;
            for i in 0..=2000 {
                let t = i as f64 / 2000.0;
                let segment = &segments
                    [((t * segments.len() as f64).floor() as usize).min(segments.len() - 1)];
                let local = (t * 0.22 - segment.offset).clamp(0.0, step);
                let [a, b, c, d] = segment.coefficients.map(|v| v as f64);
                let actual = a + local * (b + local * (c + local * d));
                assert!(
                    (actual - curve.sample(t).0).abs() < 0.0001,
                    "{curve:?} {t}: {actual}"
                );
            }
            assert_eq!(curve.sample(0.0).0, 0.0);
            assert_eq!(curve.sample(1.0).0, 1.0);
        }
    }

    #[test]
    fn motion_endpoints_and_durations_are_the_specification() {
        assert_eq!(
            Motion::dialog(true).from,
            Pose {
                x: 0.0,
                y: 8.0,
                scale: 0.96,
                opacity: 0.0
            }
        );
        assert_eq!(
            Motion::dialog(false).to,
            Pose {
                x: 0.0,
                y: 4.0,
                scale: 0.98,
                opacity: 0.0
            }
        );
        assert_eq!(
            Motion::toast(true).from,
            Pose {
                x: 0.0,
                y: 16.0,
                scale: 0.95,
                opacity: 0.0
            }
        );
        assert_eq!(
            Motion::toast(false).to,
            Pose {
                x: 0.0,
                y: 8.0,
                scale: 0.96,
                opacity: 0.0
            }
        );
        assert_eq!(Motion::press(true).to.scale, 0.97);
        assert_eq!(Motion::slide(30.0).to, Pose::IDENTITY);
        assert_eq!(
            [
                Motion::backdrop(true).milliseconds,
                Motion::dialog(true).milliseconds,
                Motion::dialog(false).milliseconds,
                Motion::toast(true).milliseconds,
                Motion::toast(false).milliseconds
            ],
            [200, 220, 150, 220, 160]
        );
    }

    #[test]
    fn search_and_keyboard_wrap_handle_unicode_empty_and_multiple_terms() {
        let commands = [
            PaletteCommand {
                id: 1,
                category: "일정",
                title: "새 일정",
                keywords: "new event",
                shortcut: "",
            },
            PaletteCommand {
                id: 2,
                category: "보기",
                title: "오늘",
                keywords: "TODAY",
                shortcut: "",
            },
        ];
        assert_eq!(search_commands(&commands, "  "), vec![0, 1]);
        assert_eq!(search_commands(&commands, "새 일정"), vec![0]);
        assert_eq!(search_commands(&commands, "today"), vec![1]);
        assert!(search_commands(&commands, "없는 명령").is_empty());
        assert_eq!(wrap_selection(Some(0), -1, 2), Some(1));
        assert_eq!(wrap_selection(Some(1), 1, 2), Some(0));
        assert_eq!(wrap_selection(None, 1, 0), None);
    }

    #[test]
    fn palette_toggle_does_not_repeat_or_steal_unmodified_typing() {
        assert!(palette_shortcut(b'K' as u16, true, false));
        assert!(!palette_shortcut(b'K' as u16, false, false));
        assert!(!palette_shortcut(b'K' as u16, true, true));
        assert!(!palette_shortcut(b'J' as u16, true, false));
    }
}
