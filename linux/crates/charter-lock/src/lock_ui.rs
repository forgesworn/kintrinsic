//! Pure UI model for the humane lock: where the action buttons sit and which
//! action a click maps to. No X11, no I/O — so the layout + hit-testing is
//! fully unit-tested; `main.rs` only draws these rects and execs the result.

/// A sanctioned escape the locked child may always take, plus the humane ask
/// (offered only when a guardian is paired to receive it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Logout,
    Shutdown,
    Suspend,
    AskMoreTime,
}

/// A screen rectangle (X11 coordinates: origin top-left, pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    fn contains(&self, x: i16, y: i16) -> bool {
        x >= self.x
            && y >= self.y
            && (x as i32) < self.x as i32 + self.width as i32
            && (y as i32) < self.y as i32 + self.height as i32
    }
}

/// A drawn button: its rect, its label, and the action it triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub rect: Rect,
    pub label: &'static str,
    pub action: Action,
}

const BTN_W: u16 = 180;
const BTN_H: u16 = 48;
const BTN_GAP: u16 = 20;

/// The centered dialog card the message + buttons live in (the rest of the
/// screen shows the child's dimmed desktop behind it).
pub const CARD_W: u16 = 680;
pub const CARD_H: u16 = 300;
/// Vertical room each extra info line (schedule/usage) adds to the card.
pub const LINE_STEP: u16 = 26;
/// Vertical room the "Ask for more time" primary button adds when offered.
pub const ASK_STEP: u16 = BTN_H + 20;
/// Bottom padding between the button row and the card's lower edge.
const CARD_BTN_INSET: u16 = 36;

/// Where the dialog card sits: centered on the screen, growing downward-room
/// for `extra_lines` schedule/usage lines and (when a guardian is paired to
/// hear it) the "Ask for more time" primary button.
pub fn card_rect(screen_w: u16, screen_h: u16, extra_lines: u16, can_ask: bool) -> Rect {
    let h = CARD_H + extra_lines * LINE_STEP + if can_ask { ASK_STEP } else { 0 };
    Rect {
        x: (((screen_w as i32) - (CARD_W as i32)) / 2).max(0) as i16,
        y: (((screen_h as i32) - (h as i32)) / 2).max(0) as i16,
        width: CARD_W,
        height: h,
    }
}

/// The buttons: the "Ask for more time" primary (paired only) as a wide row
/// above the three system escapes along the card's bottom.
pub fn button_layout(screen_w: u16, screen_h: u16, extra_lines: u16, can_ask: bool) -> Vec<Button> {
    let specs = [
        ("Log out", Action::Logout),
        ("Shut down", Action::Shutdown),
        ("Suspend", Action::Suspend),
    ];
    let card = card_rect(screen_w, screen_h, extra_lines, can_ask);
    let n = specs.len() as u16;
    let total_w = n * BTN_W + (n - 1) * BTN_GAP;
    let start_x = card.x + (((card.width as i32) - (total_w as i32)) / 2).max(0) as i16;
    let y = card.y + (card.height - BTN_H - CARD_BTN_INSET) as i16;
    let mut buttons = Vec::with_capacity(4);
    if can_ask {
        buttons.push(Button {
            rect: Rect {
                x: start_x,
                y: y - ASK_STEP as i16,
                width: total_w,
                height: BTN_H,
            },
            label: "Ask for more time",
            action: Action::AskMoreTime,
        });
    }
    buttons.extend(specs.iter().enumerate().map(|(i, (label, action))| {
        let x = start_x + (i as i16) * (BTN_W + BTN_GAP) as i16;
        Button {
            rect: Rect {
                x,
                y,
                width: BTN_W,
                height: BTN_H,
            },
            label,
            action: *action,
        }
    }));
    buttons
}

/// The action for a click at `(x, y)`, or `None` if it missed every button.
pub fn hit_test(buttons: &[Button], x: i16, y: i16) -> Option<Action> {
    buttons
        .iter()
        .find(|b| b.rect.contains(x, y))
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_places_three_buttons_in_a_centered_row_inside_the_card() {
        let b = button_layout(1920, 1080, 0, false);
        let card = card_rect(1920, 1080, 0, false);
        assert_eq!(b.len(), 3);
        assert_eq!(
            b.iter().map(|x| x.action).collect::<Vec<_>>(),
            vec![Action::Logout, Action::Shutdown, Action::Suspend]
        );
        // The card is centered and fully on screen.
        assert!(card.x > 0 && card.y > 0);
        assert!((card.x as i32 + card.width as i32) <= 1920);
        // Every button sits fully INSIDE the card (what is drawn is clickable).
        for btn in &b {
            assert!(btn.rect.x >= card.x);
            assert!(
                (btn.rect.x as i32 + btn.rect.width as i32) <= (card.x as i32 + card.width as i32)
            );
            assert!(btn.rect.y >= card.y);
            assert!(
                (btn.rect.y as i32 + btn.rect.height as i32)
                    <= (card.y as i32 + card.height as i32)
            );
        }
        // The row is centered in the card: left margin == right margin (±1px).
        let left = b[0].rect.x as i32 - card.x as i32;
        let right =
            (card.x as i32 + card.width as i32) - (b[2].rect.x as i32 + b[2].rect.width as i32);
        assert!(
            (left - right).abs() <= 1,
            "row not centered: {left} vs {right}"
        );
    }

    #[test]
    fn hit_test_maps_a_click_inside_a_button_to_its_action() {
        let b = button_layout(1920, 1080, 0, false);
        let mid = |r: &Rect| (r.x + (r.width / 2) as i16, r.y + (r.height / 2) as i16);
        let (lx, ly) = mid(&b[0].rect);
        assert_eq!(hit_test(&b, lx, ly), Some(Action::Logout));
        let (sx, sy) = mid(&b[2].rect);
        assert_eq!(hit_test(&b, sx, sy), Some(Action::Suspend));
        // A click in dead space hits nothing.
        assert_eq!(hit_test(&b, 5, 5), None);
    }
}
