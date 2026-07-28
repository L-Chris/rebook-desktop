#[derive(Clone)]
pub(crate) struct PendingTask<T> {
    pub id: u64,
    pub payload: T,
}

#[derive(Debug)]
pub(crate) struct TaskResult<T> {
    pub id: u64,
    pub result: Result<T, String>,
}

pub(crate) struct TaskSlot<T> {
    pub pending: Option<PendingTask<T>>,
    in_flight: Option<PendingTask<T>>,
    next_id: u64,
}

impl<T> Default for TaskSlot<T> {
    fn default() -> Self {
        Self {
            pending: None,
            in_flight: None,
            next_id: 1,
        }
    }
}

impl<T> TaskSlot<T> {
    pub fn is_pending(&self) -> bool {
        self.pending.is_some() || self.in_flight.is_some()
    }

    pub fn begin(&mut self, payload: T) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pending = Some(PendingTask { id, payload });
        id
    }

    pub fn take_pending(&mut self) -> Option<PendingTask<T>>
    where
        T: Clone,
    {
        let request = self.pending.take()?;
        self.in_flight = Some(request.clone());
        Some(request)
    }

    pub fn complete(&mut self, id: u64) -> Option<T> {
        if self.in_flight.as_ref().map(|request| request.id) != Some(id) {
            return None;
        }
        self.in_flight.take().map(|request| request.payload)
    }

    pub fn cancel(&mut self) {
        self.pending = None;
        self.in_flight = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_completion_cannot_clear_the_current_request() {
        let mut slot = TaskSlot::default();
        let first = slot.begin("first");
        let _ = slot.take_pending();
        slot.cancel();
        let second = slot.begin("second");
        let _ = slot.take_pending();

        assert_eq!(slot.complete(first), None);
        assert!(slot.is_pending());
        assert_eq!(slot.complete(second), Some("second"));
        assert!(!slot.is_pending());
    }
}
