//! Sets of UTF-16 units, as sorted, disjoint, non-adjacent ranges.

/// A set of UTF-16 units (0 to 0xFFFF, surrogates included).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitSet {
    ranges: Vec<(u16, u16)>,
}

impl UnitSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn full() -> Self {
        Self {
            ranges: vec![(0, 0xFFFF)],
        }
    }

    pub fn unit(u: u16) -> Self {
        Self {
            ranges: vec![(u, u)],
        }
    }

    /// The units in `ranges` (inclusive pairs, in any order).
    pub fn from_ranges(ranges: impl IntoIterator<Item = (u16, u16)>) -> Self {
        let mut rs: Vec<(u16, u16)> = ranges.into_iter().filter(|(a, b)| a <= b).collect();
        rs.sort_unstable();
        let mut out: Vec<(u16, u16)> = Vec::with_capacity(rs.len());
        for (a, b) in rs {
            match out.last_mut() {
                Some(last) if u32::from(a) <= u32::from(last.1) + 1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        Self { ranges: out }
    }

    pub fn ranges(&self) -> &[(u16, u16)] {
        &self.ranges
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.ranges == [(0, 0xFFFF)]
    }

    pub fn contains(&self, u: u16) -> bool {
        self.ranges
            .binary_search_by(|&(a, b)| {
                if b < u {
                    std::cmp::Ordering::Less
                } else if a > u {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    pub fn union(&self, other: &Self) -> Self {
        Self::from_ranges(self.ranges.iter().chain(&other.ranges).copied())
    }

    /// Every unit not in the set.
    pub fn complement(&self) -> Self {
        let mut out = Vec::new();
        let mut next: u32 = 0;
        for &(a, b) in &self.ranges {
            if u32::from(a) > next {
                out.push((next as u16, a - 1));
            }
            next = u32::from(b) + 1;
        }
        if next <= 0xFFFF {
            out.push((next as u16, 0xFFFF));
        }
        Self { ranges: out }
    }

    pub fn intersect(&self, other: &Self) -> Self {
        self.complement().union(&other.complement()).complement()
    }

    pub fn minus(&self, other: &Self) -> Self {
        self.intersect(&other.complement())
    }

    /// Whether any unit is in both.
    pub fn overlaps(&self, other: &Self) -> bool {
        !self.intersect(other).is_empty()
    }

    pub fn len(&self) -> usize {
        self.ranges
            .iter()
            .map(|&(a, b)| usize::from(b) - usize::from(a) + 1)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_and_complement() {
        let s = UnitSet::from_ranges([(5, 9), (1, 3), (4, 4), (20, 30)]);
        assert_eq!(s.ranges(), &[(1, 9), (20, 30)]);
        assert_eq!(s.complement().ranges(), &[(0, 0), (10, 19), (31, 0xFFFF)]);
        assert_eq!(s.complement().complement(), s);
        assert!(UnitSet::full().complement().is_empty());
        assert!(s.contains(9) && !s.contains(10) && s.contains(25));
        let t = UnitSet::from_ranges([(8, 22)]);
        assert_eq!(s.intersect(&t).ranges(), &[(8, 9), (20, 22)]);
        assert_eq!(s.minus(&t).ranges(), &[(1, 7), (23, 30)]);
        assert_eq!(s.len(), 9 + 11);
    }
}
