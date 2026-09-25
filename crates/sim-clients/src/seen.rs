/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use std::collections::{HashMap, VecDeque};

pub enum Seen {
    First,
    Repeat { after_ack: bool },
}

pub struct SeenWindow {
    capacity: usize,
    order: VecDeque<String>,
    acked: HashMap<String, bool>,
}

impl SeenWindow {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            acked: HashMap::with_capacity(capacity),
        }
    }

    pub fn observe(&mut self, notification_id: &str) -> Seen {
        if let Some(after_ack) = self.acked.get(notification_id) {
            return Seen::Repeat {
                after_ack: *after_ack,
            };
        }
        while self.order.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.acked.remove(&evicted);
            }
        }
        self.order.push_back(notification_id.to_string());
        self.acked.insert(notification_id.to_string(), false);
        Seen::First
    }

    pub fn mark_acked(&mut self, notification_id: &str) {
        if let Some(acked) = self.acked.get_mut(notification_id) {
            *acked = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_first(seen: Seen) -> bool {
        matches!(seen, Seen::First)
    }

    #[test]
    fn a_repeat_before_the_ack_is_labelled_as_such() {
        let mut window = SeenWindow::new(8);
        assert!(is_first(window.observe("a")));
        assert!(matches!(
            window.observe("a"),
            Seen::Repeat { after_ack: false }
        ));
    }

    #[test]
    fn a_repeat_after_the_ack_is_labelled_as_such() {
        let mut window = SeenWindow::new(8);
        window.observe("a");
        window.mark_acked("a");
        assert!(matches!(
            window.observe("a"),
            Seen::Repeat { after_ack: true }
        ));
    }

    #[test]
    fn the_window_evicts_oldest_first_and_stays_bounded() {
        let mut window = SeenWindow::new(2);
        window.observe("a");
        window.observe("b");
        window.observe("c");

        assert!(
            is_first(window.observe("a")),
            "'a' should have been evicted"
        );
        assert!(!is_first(window.observe("c")), "'c' is still in the window");
        assert!(window.acked.len() <= 2);
    }

    #[test]
    fn marking_an_id_the_window_never_saw_is_a_no_op() {
        let mut window = SeenWindow::new(2);
        window.mark_acked("absent");
        assert!(is_first(window.observe("absent")));
    }
}
