//! Recognises a two-finger sideways swipe in the stream of trackpad scroll
//! events, which carry no "gesture began" or "gesture ended" marks.

/// Silence after which the next event starts a new gesture.
const GAP: f64 = 0.09;
/// Travel after which a gesture is taken as sideways or as plain scrolling.
const DECIDE: f32 = 10.0;
/// Sideways travel that makes a swipe.
const TRAVEL: f32 = 70.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Phase {
    #[default]
    Undecided,
    Sideways,
    /// Sideways over something that scrolls that way: the gesture is left to it.
    Scrolling,
    /// Scrolling up or down, or a swipe already reported: nothing more until the gesture ends.
    Ignored,
}

#[derive(Default)]
pub struct Swipe {
    phase: Phase,
    last: f64,
    x: f32,
    y: f32,
    /// Smallest event since the gesture was set aside, to tell the fading glide from a new push.
    low: f32,
}

impl Swipe {
    /// True while the current gesture scrolls what is under the pointer sideways.
    pub fn scrolling(&self) -> bool {
        self.phase == Phase::Scrolling
    }

    /// Feeds one scroll event (`dx`, `dy` in points, moving the content) seen at `now` seconds.
    /// `scrolls(right)` tells whether the content under the pointer can still move that way;
    /// a swipe only counts where it cannot.
    /// Returns `Some(forward)` once per swipe: content pushed right goes back, left goes forward.
    pub fn feed(&mut self, now: f64, dx: f32, dy: f32, scrolls: impl Fn(bool) -> bool) -> Option<bool> {
        let speed = dx.abs().max(dy.abs());
        // The glide after the fingers lift only slows down: a jump in speed is a new push.
        let set_aside = matches!(self.phase, Phase::Ignored | Phase::Scrolling);
        let pushed_again = set_aside && speed >= 6.0 && speed >= self.low * 3.0;
        if now - self.last > GAP || pushed_again {
            *self = Swipe::default();
        }
        self.last = now;
        if matches!(self.phase, Phase::Ignored | Phase::Scrolling) {
            self.low = self.low.min(speed);
            return None;
        }
        self.x += dx;
        self.y += dy;
        let (ax, ay) = (self.x.abs(), self.y.abs());
        if self.phase == Phase::Undecided && ax.max(ay) >= DECIDE && ax > ay * 2.0 {
            if scrolls(self.x > 0.0) {
                self.phase = Phase::Scrolling;
                self.low = speed;
                return None;
            }
            self.phase = Phase::Sideways;
        }
        let swiped = self.phase == Phase::Sideways && ax >= TRAVEL && ax >= ay;
        // Scrolling, a swipe that turned away, or a swipe now reported: wait for the next gesture.
        let lost = if self.phase == Phase::Sideways { ay > ax } else { ax.max(ay) >= DECIDE };
        if swiped || lost {
            self.phase = Phase::Ignored;
            self.low = speed;
        }
        swiped.then_some(self.x < 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds events 1/60 s apart starting at `t0` and collects what was reported.
    fn run(s: &mut Swipe, t0: f64, events: &[(f32, f32)]) -> Vec<bool> {
        events.iter().enumerate().filter_map(|(i, &(x, y))| s.feed(t0 + i as f64 / 60.0, x, y, |_| false)).collect()
    }

    /// A push followed by the fading glide the system adds.
    fn swipe(sign: f32) -> Vec<(f32, f32)> {
        let mut v: Vec<(f32, f32)> = [4.0, 9.0, 18.0, 26.0, 30.0, 24.0].iter().map(|d| (d * sign, 0.5)).collect();
        let mut d = 20.0;
        while d > 0.5 {
            v.push((d * sign, 0.0));
            d *= 0.85;
        }
        v
    }

    #[test]
    fn one_swipe_reports_once() {
        let mut s = Swipe::default();
        assert_eq!(run(&mut s, 1.0, &swipe(1.0)), [false], "content pushed right goes back");
        assert_eq!(run(&mut s, 3.0, &swipe(-1.0)), [true]);
    }

    #[test]
    fn scrolling_a_list_is_not_a_swipe() {
        let mut s = Swipe::default();
        let scroll: Vec<(f32, f32)> = (0..80).map(|i| (if i > 10 { 6.0 } else { 1.0 }, 20.0)).collect();
        assert!(run(&mut s, 1.0, &scroll).is_empty());
        // Drifting sideways at the end of a scroll does not count either.
        let drift: Vec<(f32, f32)> = (0..40).map(|i| (if i > 5 { 30.0 } else { 0.0 }, if i > 5 { 0.0 } else { 20.0 })).collect();
        assert!(run(&mut s, 5.0, &drift).is_empty());
    }

    #[test]
    fn short_nudge_is_ignored() {
        let mut s = Swipe::default();
        // A first event that leans the wrong way does not decide anything.
        let mut wobbly = vec![(0.2, 0.6)];
        wobbly.extend(swipe(-1.0));
        assert_eq!(run(&mut s, 0.5, &wobbly), [true]);
        let mut s = Swipe::default();
        assert!(run(&mut s, 1.0, &[(8.0, 0.0), (12.0, 0.0), (9.0, 0.0), (4.0, 0.0), (1.0, 0.0)]).is_empty());
    }

    #[test]
    fn scrollable_content_takes_the_gesture() {
        let mut s = Swipe::default();
        let mut seen = Vec::new();
        // The columns can still move right: the first push scrolls them. At their
        // edge the second push is a swipe.
        for (n, room) in [(0, true), (1, false)] {
            for (i, &(x, y)) in swipe(1.0).iter().enumerate() {
                seen.extend(s.feed(n as f64 * 3.0 + i as f64 / 60.0, x, y, |right| right && room));
                assert_eq!(s.scrolling(), room && i > 0);
            }
        }
        assert_eq!(seen, [false]);
    }

    #[test]
    fn two_swipes_in_a_row_report_twice() {
        let mut s = Swipe::default();
        let mut events = swipe(1.0);
        events.extend(swipe(1.0));
        assert_eq!(run(&mut s, 1.0, &events), [false, false]);
    }
}
