use super::Ms;

/// Unix milliseconds. Wall clock, for display and ordering of records. Durations of media use
/// [`Ms`], never this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

/// A span of recorded media with `start <= end`. Fields are private so no other value exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimeRange {
    start: Ms,
    end: Ms,
}

impl TimeRange {
    pub fn new(start: Ms, end: Ms) -> Option<TimeRange> {
        (start <= end).then_some(TimeRange { start, end })
    }

    pub fn start(&self) -> Ms {
        self.start
    }

    pub fn end(&self) -> Ms {
        self.end
    }

    pub fn len(&self) -> Ms {
        Ms(self.end.0 - self.start.0)
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Start inclusive, end exclusive.
    pub fn contains(&self, at: Ms) -> bool {
        self.start <= at && at < self.end
    }

    pub fn overlap(&self, other: &TimeRange) -> Ms {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        Ms(end.0.saturating_sub(start.0))
    }

    /// Moves the range later by `by`, for example from clip time to take time.
    pub fn shifted(&self, by: Ms) -> TimeRange {
        TimeRange {
            start: Ms(self.start.0.saturating_add(by.0)),
            end: Ms(self.end.0.saturating_add(by.0)),
        }
    }
}

/// Progress in `0.0..=1.0`. Out-of-range values clamp and NaN becomes zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Fraction(f32);

impl Fraction {
    pub fn new(value: f32) -> Fraction {
        Fraction(if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 1.0)
        })
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: u64, b: u64) -> TimeRange {
        TimeRange::new(Ms(a), Ms(b)).unwrap()
    }

    #[test]
    fn construction_requires_start_not_after_end() {
        assert!(TimeRange::new(Ms(5), Ms(5)).is_some());
        assert!(TimeRange::new(Ms(5), Ms(6)).is_some());
        assert!(TimeRange::new(Ms(6), Ms(5)).is_none());
    }

    #[test]
    fn overlap_table() {
        let cases = [
            (r(0, 10), r(5, 15), 5),
            (r(0, 10), r(10, 20), 0),
            (r(0, 10), r(20, 30), 0),
            (r(0, 100), r(40, 60), 20),
            (r(7, 7), r(0, 10), 0),
        ];
        for (a, b, want) in cases {
            assert_eq!(a.overlap(&b), Ms(want), "{a:?} {b:?}");
            assert_eq!(b.overlap(&a), Ms(want), "{b:?} {a:?}");
        }
    }

    #[test]
    fn contains_is_half_open_and_shift_keeps_length() {
        let range = r(10, 20);
        assert!(range.contains(Ms(10)));
        assert!(!range.contains(Ms(20)));
        assert_eq!(range.shifted(Ms(100)), r(110, 120));
        assert_eq!(range.len(), Ms(10));
        assert!(r(3, 3).is_empty());
    }

    #[test]
    fn fraction_clamps() {
        assert_eq!(Fraction::new(-1.0).get(), 0.0);
        assert_eq!(Fraction::new(2.0).get(), 1.0);
        assert_eq!(Fraction::new(f32::NAN).get(), 0.0);
        assert_eq!(Fraction::new(0.25).get(), 0.25);
    }
}
