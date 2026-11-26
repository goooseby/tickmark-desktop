pub const IDLE_MS: i64 = 120_000;
pub const TRANSIENT_MS: i64 = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum State {
    Active = 0,
    Idle = 1,
    Offline = 2,
    Gap = 3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub page: Option<String>,
    pub transient: bool,
}

#[derive(Clone, Debug)]
pub enum Frame {
    Online { target: Option<Target>, media_exempt: bool },
    Offline,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub start: i64,
    pub end: i64,
    pub state: State,
    pub media_exempt: bool,
    pub target: Option<Target>,
}

pub struct Engine {
    cursor: i64,
    online_since: i64,
    input_at: Option<i64>,
    deadline: i64,
    frame: Frame,
    transient_since: i64,
    prior_target: Option<Target>,
    buffered: Vec<Slice>,
}

impl Engine {
    pub fn new(now: i64) -> Self {
        Self {
            cursor: now,
            online_since: now,
            input_at: None,
            deadline: now,
            frame: Frame::Unknown,
            transient_since: now,
            prior_target: None,
            buffered: vec![],
        }
    }

    pub(crate) fn current_frame(&self) -> Frame {
        self.frame.clone()
    }

    pub fn observe(&mut self, now: i64, input: Option<i64>, next: Frame) -> Vec<Slice> {
        assert!(now >= self.cursor);
        let mut output = vec![];
        let was_online = matches!(self.frame, Frame::Online { .. });
        let is_online = matches!(next, Frame::Online { .. });
        if was_online
            && let Some(at) = input
            && at > self.online_since
            && at <= now
            && self.input_at.is_none_or(|last| at > last)
        {
            self.emit(self.cursor.max(at), &mut output);
            self.input_at = Some(at);
            self.deadline = at + IDLE_MS;
        }
        self.emit(now, &mut output);

        let transient = self.target().is_some_and(|target| target.transient);
        let next_target = match &next {
            Frame::Online { target, .. } => target.as_ref(),
            _ => None,
        };
        let next_transient = next_target.is_some_and(|target| target.transient);
        if transient && !next_transient {
            let merge = now - self.transient_since < TRANSIENT_MS && next_target.is_some();
            self.flush_transient(merge, &mut output);
        }
        if next_transient && !transient {
            self.transient_since = now;
            self.prior_target = self.target().cloned().map(|mut target| {
                target.page = None;
                target
            });
        }
        if !was_online || !is_online {
            self.online_since = now;
            self.input_at = None;
            self.deadline = now;
        }
        self.frame = next;
        output
    }

    pub fn finish(&mut self, now: i64, input: Option<i64>) -> Vec<Slice> {
        let mut output = self.observe(now, input, Frame::Unknown);
        self.flush_transient(false, &mut output);
        output
    }

    fn target(&self) -> Option<&Target> {
        match &self.frame {
            Frame::Online { target, .. } => target.as_ref(),
            _ => None,
        }
    }

    fn emit(&mut self, end: i64, output: &mut Vec<Slice>) {
        let start = self.cursor;
        if start == end {
            return;
        }
        self.cursor = end;
        match &self.frame {
            Frame::Offline => output.push(Slice { start, end, state: State::Offline, media_exempt: false, target: None }),
            Frame::Unknown => output.push(Slice { start, end, state: State::Gap, media_exempt: false, target: None }),
            Frame::Online { target, media_exempt } => {
                let target = target.clone();
                let transient = target.as_ref().is_some_and(|value| value.transient);
                let exempt = *media_exempt && self.input_at.is_some() && target.is_some() && !transient;
                if exempt {
                    self.deadline = end + IDLE_MS;
                }
                let active_end = self.deadline.clamp(start, end);
                let sink = if transient { &mut self.buffered } else { &mut *output };
                if active_end > start {
                    let state = if target.is_some() { State::Active } else { State::Gap };
                    sink.push(Slice { start, end: active_end, state, media_exempt: exempt, target });
                }
                if active_end < end {
                    sink.push(Slice { start: active_end, end, state: State::Idle, media_exempt: false, target: None });
                }
                if transient && end - self.transient_since >= TRANSIENT_MS {
                    self.flush_transient(false, output);
                }
            }
        }
    }

    fn flush_transient(&mut self, merge: bool, output: &mut Vec<Slice>) {
        for mut slice in self.buffered.drain(..) {
            if merge && slice.state == State::Active && self.prior_target.is_some() {
                slice.target = self.prior_target.clone();
            }
            output.push(slice);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: &str, transient: bool, media_exempt: bool) -> Frame {
        Frame::Online {
            target: Some(Target { id: id.into(), name: id.into(), page: None, transient }),
            media_exempt,
        }
    }

    #[test]
    fn input_at_deadline_is_continuous_and_late_input_does_not_backfill() {
        let normal = frame("editor", false, false);
        let mut engine = Engine::new(0);
        engine.observe(0, None, normal.clone());
        engine.observe(1, Some(1), normal.clone());
        let first = engine.observe(120_001, Some(120_001), normal.clone());
        assert_eq!(first[0].end - first[0].start, IDLE_MS);
        let next = engine.observe(300_001, Some(300_001), normal);
        assert_eq!(next[0].end, 240_001);
        assert_eq!(next[1].state, State::Idle);
        assert_eq!(next[1].end, 300_001);
    }

    #[test]
    fn short_shell_overlay_belongs_to_prior_app_but_lock_cuts_it() {
        let normal = frame("editor", false, false);
        let shell = frame("shell", true, false);
        let mut engine = Engine::new(0);
        engine.observe(0, None, normal.clone());
        engine.observe(1, Some(1), normal.clone());
        engine.observe(1_001, None, shell.clone());
        let folded = engine.observe(2_001, None, normal);
        assert_eq!(folded[0].target.as_ref().unwrap().id, "editor");
        engine.observe(2_501, None, shell);
        let locked = engine.observe(3_001, None, Frame::Offline);
        assert_eq!(locked[0].target.as_ref().unwrap().id, "shell");
    }

    #[test]
    fn media_exempt_holds_existing_timer_but_cannot_unlock_it() {
        let video = frame("video", false, true);
        let mut engine = Engine::new(0);
        engine.observe(0, None, video.clone());
        engine.observe(1, Some(1), video.clone());
        let played = engine.observe(900_001, None, Frame::Offline);
        assert_eq!(played[0].state, State::Active);
        engine.observe(905_001, Some(903_000), video.clone());
        let unattended = engine.observe(906_001, None, video);
        assert_eq!(unattended[0].state, State::Idle);
    }
}
