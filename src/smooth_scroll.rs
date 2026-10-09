use gpui::{
    App, DispatchPhase, EntityId, HitboxBehavior, IntoElement, ListState, MouseDownEvent, Pixels,
    Point, ScrollWheelEvent, Size, Styled, Window, canvas, point, px,
};
use gpui_component::scroll::ScrollbarHandle;
use std::{cell::RefCell, rc::Rc, time::Instant};

#[derive(Default)]
struct Motion {
    remaining: f32,
    last_frame: Option<Instant>,
    scheduled: bool,
    generation: u64,
}

#[derive(Clone, Default)]
pub struct SmoothScroll(Rc<RefCell<Motion>>);

impl SmoothScroll {
    pub fn cancel(&self) {
        let mut motion = self.0.borrow_mut();
        motion.remaining = 0.;
        motion.last_frame = None;
        motion.scheduled = false;
        motion.generation = motion.generation.wrapping_add(1);
    }

    pub fn handle(&self, list: &ListState) -> SmoothListHandle {
        SmoothListHandle {
            list: list.clone(),
            motion: self.clone(),
        }
    }

    pub fn layer(&self, list: ListState) -> impl IntoElement {
        let motion = self.clone();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let owner = window.current_view();
                let mouse_motion = motion.clone();
                let mouse_hitbox = hitbox.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, window, _| {
                    if phase == DispatchPhase::Capture && mouse_hitbox.is_hovered(window) {
                        mouse_motion.cancel();
                    }
                });
                // Capture before List's default wheel handler so the delta is applied once.
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                        return;
                    }
                    if event.delta.precise() {
                        // Touchpads already provide pixel-level motion and platform momentum.
                        motion.cancel();
                        return;
                    }
                    let distance = -f32::from(event.delta.pixel_delta(px(56.)).y);
                    if distance == 0. {
                        return;
                    }
                    cx.stop_propagation();
                    motion.add(distance, &list, owner, window);
                });
            },
        )
        .absolute()
        .inset_0()
    }

    fn add(&self, distance: f32, list: &ListState, owner: EntityId, window: &Window) {
        let mut motion = self.0.borrow_mut();
        // Reversing the wheel cancels the old direction instead of fighting its momentum.
        if motion.remaining.signum() != distance.signum() {
            motion.remaining = 0.;
        }
        let limit = f32::from(list.viewport_bounds().size.height).max(200.) * 1.5;
        motion.remaining = (motion.remaining + distance).clamp(-limit, limit);
        if motion.scheduled {
            return;
        }
        motion.scheduled = true;
        motion.last_frame = Some(Instant::now());
        let generation = motion.generation;
        drop(motion);
        self.schedule(list.clone(), owner, generation, window);
    }

    fn schedule(&self, list: ListState, owner: EntityId, generation: u64, window: &Window) {
        let motion = self.clone();
        window.on_next_frame(move |window, cx| motion.tick(list, owner, generation, window, cx));
    }

    fn tick(
        &self,
        list: ListState,
        owner: EntityId,
        generation: u64,
        window: &Window,
        cx: &mut App,
    ) {
        let now = Instant::now();
        let mut motion = self.0.borrow_mut();
        if motion.generation != generation {
            return;
        }
        let elapsed = motion.last_frame.replace(now).map_or(1. / 60., |previous| {
            now.duration_since(previous).as_secs_f32().min(0.05)
        });
        // Frame-rate independent, short easing: most of a wheel step settles in ~120 ms.
        let step = if motion.remaining.abs() < 0.5 {
            motion.remaining
        } else {
            motion.remaining * (1. - (-elapsed / 0.035).exp())
        };
        let offset = -f32::from(list.scroll_px_offset_for_scrollbar().y);
        let maximum = f32::from(list.max_offset_for_scrollbar().height);
        let next = (offset + step).clamp(0., maximum);
        let consumed = next - offset;
        list.set_offset_from_scrollbar(point(px(0.), px(-next)));
        motion.remaining -= consumed;
        if (step - consumed).abs() > 0.5 || motion.remaining.abs() < 0.1 || consumed.abs() < 0.01 {
            motion.remaining = 0.;
            motion.scheduled = false;
        }
        let again = motion.scheduled;
        drop(motion);
        cx.notify(owner);
        if again {
            self.schedule(list, owner, generation, window);
        }
    }
}

#[derive(Clone)]
pub struct SmoothListHandle {
    list: ListState,
    motion: SmoothScroll,
}

impl ScrollbarHandle for SmoothListHandle {
    fn offset(&self) -> Point<Pixels> {
        self.list.scroll_px_offset_for_scrollbar()
    }
    fn set_offset(&self, offset: Point<Pixels>) {
        self.motion.cancel();
        self.list.set_offset_from_scrollbar(offset);
    }
    fn content_size(&self) -> Size<Pixels> {
        self.list.viewport_bounds().size + self.list.max_offset_for_scrollbar()
    }
    fn start_drag(&self) {
        self.motion.cancel();
        self.list.scrollbar_drag_started();
    }
    fn end_drag(&self) {
        self.list.scrollbar_drag_ended();
    }
}
