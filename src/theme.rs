use gpui::{App, px, rgb, rgba};
use gpui_component::Theme;

#[derive(Clone, Copy)]
pub struct ThemeSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub palette: [u32; 5],
    // Indices into the original palette: paper, ink, accent, secondary, highlight.
    roles: [usize; 5],
}

pub const THEMES: [ThemeSpec; 8] = [
    ThemeSpec {
        id: "default",
        name: "默认 · 鼠尾草",
        palette: [0xf6f7f1, 0xedf0e5, 0x29352b, 0x658348, 0xdde8d0],
        roles: [0, 2, 3, 1, 4],
    },
    ThemeSpec {
        id: "terracotta",
        name: "陶土奶油",
        palette: [0xf4f1de, 0xe07a5f, 0x3d405b, 0x81b29a, 0xf2cc8f],
        roles: [0, 2, 1, 3, 4],
    },
    ThemeSpec {
        id: "rose",
        name: "复古玫瑰",
        palette: [0x8c1c13, 0xbf4342, 0xe7d7c1, 0xa78a7f, 0x735751],
        roles: [2, 0, 1, 3, 4],
    },
    ThemeSpec {
        id: "glacier",
        name: "冰川蓝",
        palette: [0xbee9e8, 0x62b6cb, 0x1b4965, 0xcae9ff, 0x5fa8d3],
        roles: [3, 2, 4, 0, 1],
    },
    ThemeSpec {
        id: "forest",
        name: "林间苔绿",
        palette: [0x386641, 0x6a994e, 0xa7c957, 0xf2e8cf, 0xbc4749],
        roles: [3, 0, 1, 2, 4],
    },
    ThemeSpec {
        id: "olive",
        name: "橄榄沙丘",
        palette: [0x606c38, 0x283618, 0xfefae0, 0xdda15e, 0xbc6c25],
        roles: [2, 1, 0, 3, 4],
    },
    ThemeSpec {
        id: "coffee",
        name: "可可金棕",
        palette: [0x6f1d1b, 0xbb9457, 0x432818, 0x99582a, 0xffe6a7],
        roles: [4, 2, 0, 1, 3],
    },
    ThemeSpec {
        id: "coast",
        name: "海岸日光",
        palette: [0x227c9d, 0x17c3b2, 0xffcb77, 0xfef9ef, 0xfe6d73],
        roles: [3, 0, 1, 2, 4],
    },
];

pub fn find(id: &str) -> &'static ThemeSpec {
    THEMES
        .iter()
        .find(|theme| theme.id == id)
        .unwrap_or(&THEMES[0])
}

#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: u32,
    pub panel: u32,
    pub surface: u32,
    pub ink: u32,
    pub muted: u32,
    pub line: u32,
    pub accent: u32,
    pub selected: u32,
    pub selected_text: u32,
    pub accent_border: u32,
    pub soft: u32,
    pub hover: u32,
    pub pressed: u32,
    pub tag_bg: u32,
    pub tag_text: u32,
    pub preview_bg: u32,
    pub preview_text: u32,
    pub preview_muted: u32,
    pub primary_text: u32,
    pub overlay: u32,
}

fn mix(a: u32, b: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let a = ((a >> shift) & 255u32) as f32;
        let b = ((b >> shift) & 255u32) as f32;
        ((a + (b - a) * amount).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

impl ThemeSpec {
    pub fn colors(&self) -> Colors {
        if self.id == "default" {
            return Colors {
                bg: 0xf6f7f1,
                panel: 0xedf0e5,
                surface: 0xffffff,
                ink: 0x29352b,
                muted: 0x718064,
                line: 0xe0e6d7,
                accent: 0x658348,
                selected: 0xdde8d0,
                selected_text: 0x3f5731,
                accent_border: 0xb4c6a3,
                soft: 0xe9f0dd,
                hover: 0xe1e8d7,
                pressed: 0xd5e1c9,
                tag_bg: 0xf0f4e8,
                tag_text: 0x7c9167,
                preview_bg: 0x202c24,
                preview_text: 0xb9cba9,
                preview_muted: 0x8eab78,
                primary_text: 0xeaf3de,
                overlay: 0x17251b,
            };
        }
        let [paper, ink, accent, secondary, highlight] = self.roles.map(|i| self.palette[i]);
        // Keep text readable even in palettes whose accents are deliberately very light.
        let ink = mix(ink, 0x111111, 0.18);
        Colors {
            bg: mix(paper, 0xffffff, 0.48),
            panel: mix(paper, secondary, 0.13),
            surface: mix(paper, 0xffffff, 0.92),
            ink,
            muted: mix(ink, paper, 0.28),
            line: mix(paper, ink, 0.17),
            accent: mix(accent, ink, 0.52),
            selected: mix(paper, accent, 0.27),
            selected_text: mix(accent, ink, 0.76),
            accent_border: mix(paper, accent, 0.60),
            soft: mix(paper, secondary, 0.19),
            hover: mix(paper, highlight, 0.18),
            pressed: mix(paper, accent, 0.36),
            tag_bg: mix(paper, secondary, 0.16),
            tag_text: mix(secondary, ink, 0.75),
            preview_bg: mix(ink, 0x111111, 0.32),
            preview_text: mix(paper, 0xffffff, 0.3),
            preview_muted: mix(paper, secondary, 0.3),
            primary_text: mix(paper, 0xffffff, 0.65),
            overlay: mix(ink, 0x000000, 0.32),
        }
    }
}

pub fn apply(id: &str, cx: &mut App) {
    let colors = find(id).colors();
    let theme = Theme::global_mut(cx);
    theme.font_family = "Microsoft YaHei UI".into();
    theme.font_size = px(13.);
    theme.background = rgb(colors.bg).into();
    theme.foreground = rgb(colors.ink).into();
    theme.border = rgb(colors.line).into();
    theme.input = rgb(colors.line).into();
    theme.caret = rgb(colors.accent).into();
    theme.primary = rgb(colors.ink).into();
    theme.primary_foreground = rgb(colors.primary_text).into();
    theme.primary_hover = rgb(mix(colors.ink, colors.bg, 0.13)).into();
    theme.primary_active = rgb(mix(colors.ink, 0x000000, 0.15)).into();
    theme.ring = rgb(colors.accent_border).into();
    theme.accent = rgb(colors.hover).into();
    theme.accent_foreground = rgb(colors.ink).into();
    theme.secondary = rgb(colors.surface).into();
    theme.secondary_foreground = rgb(colors.ink).into();
    theme.secondary_hover = rgb(colors.hover).into();
    theme.secondary_active = rgb(colors.pressed).into();
    theme.muted = rgb(colors.panel).into();
    theme.muted_foreground = rgb(colors.muted).into();
    theme.switch = rgb(colors.line).into();
    theme.switch_thumb = rgb(colors.surface).into();
    theme.popover = rgb(colors.surface).into();
    theme.popover_foreground = rgb(colors.ink).into();
    theme.list = rgb(colors.surface).into();
    theme.list_active = rgb(colors.selected).into();
    theme.list_active_border = rgb(colors.accent_border).into();
    theme.list_hover = rgb(colors.hover).into();
    theme.selection = rgba((colors.accent_border << 8) | 0x80).into();
    theme.scrollbar = rgb(colors.panel).into();
    theme.scrollbar_thumb = rgb(colors.accent_border).into();
    theme.scrollbar_thumb_hover = rgb(colors.accent).into();
    theme.title_bar = rgb(colors.bg).into();
    theme.title_bar_border = rgb(colors.line).into();
    theme.overlay = rgba((colors.overlay << 8) | 0xb5).into();
    cx.refresh_windows();
}
