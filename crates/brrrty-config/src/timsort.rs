//! `Array.prototype.sort` as V8 runs it: the TimSort in V8's
//! `third_party/v8/builtins/array-sort.tq` (V8 14.6.202, the version in Node
//! 26), ported step for step. V8's file is itself a port of CPython's
//! `listobject.c`; see NOTICE.
//!
//! JavaScript only promises a sorted result for a comparator that is a
//! consistent ordering. For any other (`(a, b) => a > b` returns `true` or
//! `false`, never a negative number) the result is whatever V8's algorithm
//! gives, so this makes the same comparator calls, in the same order, with
//! the same arguments, and moves items the same way: binary insertion alone
//! under 8 items, and otherwise run detection, binary insertion up to the
//! minimum run length, the run-stack invariants,
//! galloping with V8's `minGallop` adjustment, and merging from the low or
//! the high end. `tests/probes/sort.json` (made by `tools/gen-sort-probes.cjs`
//! in Node) pins the result and the number of comparator calls for lengths 0
//! to 2000 and comparators that are consistent, inconsistent, random and
//! throwing.

use std::cmp::Ordering;

/// Wins in a row before a merge starts galloping (`kMinGallopWins`).
const MIN_GALLOP_WINS: isize = 7;

/// Sorts `a` in place, as V8 sorts the work array of `Array.prototype.sort`
/// (`undefined` already removed). `cmp(x, y)` is the comparator's result:
/// `Less` for a negative number, anything else for zero, a positive number
/// or `NaN`. V8 only ever asks whether the result is negative. An error
/// from `cmp` stops the sort and is returned; `a` is then in some partial
/// order, as V8's work array would be.
pub fn sort<T: Clone, E>(
    a: &mut [T],
    cmp: &mut dyn FnMut(&T, &T) -> Result<Ordering, E>,
) -> Result<(), E> {
    let length = a.len() as isize;
    if length < 2 {
        return Ok(());
    }
    let mut s = State {
        a,
        cmp,
        min_gallop: MIN_GALLOP_WINS,
        runs: Vec::new(),
    };
    // `ArrayTimSort`: "Faster for small arrays."
    if length < 8 {
        return s.binary_insertion_sort(0, 0, length);
    }
    let mut remaining = length;
    let mut low = 0;
    let min_run = min_run_length(remaining);
    while remaining != 0 {
        let mut run = s.count_and_make_run(low, low + remaining)?;
        if run < min_run {
            let forced = min_run.min(remaining);
            s.binary_insertion_sort(low, low + run, low + forced)?;
            run = forced;
        }
        s.runs.push((low, run));
        s.merge_collapse()?;
        low += run;
        remaining -= run;
    }
    s.merge_force_collapse()?;
    Ok(())
}

/// `ComputeMinRunLength`.
fn min_run_length(mut n: isize) -> isize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

struct State<'a, T, E> {
    a: &'a mut [T],
    cmp: &'a mut dyn FnMut(&T, &T) -> Result<Ordering, E>,
    min_gallop: isize,
    /// Pending runs: (base, length).
    runs: Vec<(isize, isize)>,
}

/// Where a gallop looks: the work array or the merge's temporary copy.
#[derive(Clone, Copy)]
enum In {
    Work,
    Temp,
}

impl<T: Clone, E> State<'_, T, E> {
    /// `order < 0` in V8's terms.
    fn less(&mut self, x: &T, y: &T) -> Result<bool, E> {
        Ok((self.cmp)(x, y)? == Ordering::Less)
    }

    fn at(&self, temp: &[T], arr: In, i: isize) -> T {
        match arr {
            In::Work => self.a[i as usize].clone(),
            In::Temp => temp[i as usize].clone(),
        }
    }

    /// `CountAndMakeRun`: the length of the run starting at `low_arg`,
    /// reversed in place when it is strictly descending.
    fn count_and_make_run(&mut self, low_arg: isize, high: isize) -> Result<isize, E> {
        let low = low_arg + 1;
        if low == high {
            return Ok(1);
        }
        let mut run = 2;
        let element_low = self.a[low as usize].clone();
        let element_low_pre = self.a[(low - 1) as usize].clone();
        let descending = self.less(&element_low, &element_low_pre)?;
        let mut previous = element_low;
        let mut idx = low + 1;
        while idx < high {
            let current = self.a[idx as usize].clone();
            let order_less = self.less(&current, &previous)?;
            if descending {
                if !order_less {
                    break;
                }
            } else if order_less {
                break;
            }
            previous = current;
            run += 1;
            idx += 1;
        }
        if descending {
            self.a[low_arg as usize..(low_arg + run) as usize].reverse();
        }
        Ok(run)
    }

    /// `BinaryInsertionSort`: `[low, start)` is sorted; insert the items
    /// of `[start, high)` one at a time.
    fn binary_insertion_sort(
        &mut self,
        low: isize,
        start_arg: isize,
        high: isize,
    ) -> Result<(), E> {
        let mut start = if low == start_arg {
            start_arg + 1
        } else {
            start_arg
        };
        while start < high {
            let mut left = low;
            let mut right = start;
            let pivot = self.a[right as usize].clone();
            while left < right {
                let mid = left + ((right - left) >> 1);
                let m = self.a[mid as usize].clone();
                if self.less(&pivot, &m)? {
                    right = mid;
                } else {
                    left = mid + 1;
                }
            }
            let mut p = start;
            while p > left {
                self.a[p as usize] = self.a[(p - 1) as usize].clone();
                p -= 1;
            }
            self.a[left as usize] = pivot;
            start += 1;
        }
        Ok(())
    }

    fn len_at(&self, n: isize) -> isize {
        self.runs[n as usize].1
    }

    /// `RunInvariantEstablished`: run n - 2 is longer than runs n - 1 and n.
    fn invariant(&self, n: isize) -> bool {
        if n < 2 {
            return true;
        }
        self.len_at(n - 2) > self.len_at(n - 1) + self.len_at(n)
    }

    /// `MergeCollapse`.
    fn merge_collapse(&mut self) -> Result<(), E> {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() as isize - 2;
            if !self.invariant(n + 1) || !self.invariant(n) {
                if self.len_at(n - 1) < self.len_at(n + 1) {
                    n -= 1;
                }
                self.merge_at(n)?;
            } else if self.len_at(n) <= self.len_at(n + 1) {
                self.merge_at(n)?;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// `MergeForceCollapse`.
    fn merge_force_collapse(&mut self) -> Result<(), E> {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() as isize - 2;
            if n > 0 && self.len_at(n - 1) < self.len_at(n + 1) {
                n -= 1;
            }
            self.merge_at(n)?;
        }
        Ok(())
    }

    /// `MergeAt`: merge runs i and i + 1.
    fn merge_at(&mut self, i: isize) -> Result<(), E> {
        let iu = i as usize;
        let (mut base_a, mut length_a) = self.runs[iu];
        let (base_b, mut length_b) = self.runs[iu + 1];
        self.runs[iu].1 = length_a + length_b;
        self.runs.remove(iu + 1);

        let key_right = self.a[base_b as usize].clone();
        let k = self.gallop_right(&[], In::Work, &key_right, base_a, length_a, 0)?;
        base_a += k;
        length_a -= k;
        if length_a == 0 {
            return Ok(());
        }
        let key_left = self.a[(base_a + length_a - 1) as usize].clone();
        length_b = self.gallop_left(&[], In::Work, &key_left, base_b, length_b, length_b - 1)?;
        if length_b == 0 {
            return Ok(());
        }
        if length_a <= length_b {
            self.merge_low(base_a, length_a, base_b, length_b)
        } else {
            self.merge_high(base_a, length_a, base_b, length_b)
        }
    }

    /// `GallopLeft`: k in 0..=length with `arr[base + k - 1] < key <= arr[base + k]`.
    fn gallop_left(
        &mut self,
        temp: &[T],
        arr: In,
        key: &T,
        base: isize,
        length: isize,
        hint: isize,
    ) -> Result<isize, E> {
        let mut last_ofs: isize = 0;
        let mut offset: isize = 1;
        let base_hint = self.at(temp, arr, base + hint);
        if self.less(&base_hint, key)? {
            let max_ofs = length - hint;
            while offset < max_ofs {
                let e = self.at(temp, arr, base + hint + offset);
                if !self.less(&e, key)? {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            last_ofs += hint;
            offset += hint;
        } else {
            let max_ofs = hint + 1;
            while offset < max_ofs {
                let e = self.at(temp, arr, base + hint - offset);
                if self.less(&e, key)? {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            let tmp = last_ofs;
            last_ofs = hint - offset;
            offset = hint - tmp;
        }
        last_ofs += 1;
        while last_ofs < offset {
            let m = last_ofs + ((offset - last_ofs) >> 1);
            let e = self.at(temp, arr, base + m);
            if self.less(&e, key)? {
                last_ofs = m + 1;
            } else {
                offset = m;
            }
        }
        Ok(offset)
    }

    /// `GallopRight`: k in 0..=length with `arr[base + k - 1] <= key < arr[base + k]`.
    fn gallop_right(
        &mut self,
        temp: &[T],
        arr: In,
        key: &T,
        base: isize,
        length: isize,
        hint: isize,
    ) -> Result<isize, E> {
        let mut last_ofs: isize = 0;
        let mut offset: isize = 1;
        let base_hint = self.at(temp, arr, base + hint);
        if self.less(key, &base_hint)? {
            let max_ofs = hint + 1;
            while offset < max_ofs {
                let e = self.at(temp, arr, base + hint - offset);
                if !self.less(key, &e)? {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            let tmp = last_ofs;
            last_ofs = hint - offset;
            offset = hint - tmp;
        } else {
            let max_ofs = length - hint;
            while offset < max_ofs {
                let e = self.at(temp, arr, base + hint + offset);
                if self.less(key, &e)? {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
                if offset <= 0 {
                    offset = max_ofs;
                }
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            last_ofs += hint;
            offset += hint;
        }
        last_ofs += 1;
        while last_ofs < offset {
            let m = last_ofs + ((offset - last_ofs) >> 1);
            let e = self.at(temp, arr, base + m);
            if self.less(key, &e)? {
                offset = m;
            } else {
                last_ofs = m + 1;
            }
        }
        Ok(offset)
    }

    /// V8's `Copy` within the work array: overlapping ranges move as
    /// `memmove` would.
    fn copy_within(&mut self, src: isize, dst: isize, len: isize) {
        if len <= 0 {
            return;
        }
        let (src, dst, len) = (src as usize, dst as usize, len as usize);
        if src < dst {
            for k in (0..len).rev() {
                self.a[dst + k] = self.a[src + k].clone();
            }
        } else {
            for k in 0..len {
                self.a[dst + k] = self.a[src + k].clone();
            }
        }
    }

    fn copy_from(&mut self, temp: &[T], src: isize, dst: isize, len: isize) {
        for k in 0..len.max(0) {
            self.a[(dst + k) as usize] = temp[(src + k) as usize].clone();
        }
    }

    /// `MergeLow`: merge forwards, with run A (the shorter) copied out.
    fn merge_low(
        &mut self,
        base_a: isize,
        length_a_arg: isize,
        base_b: isize,
        length_b_arg: isize,
    ) -> Result<(), E> {
        let mut length_a = length_a_arg;
        let mut length_b = length_b_arg;
        let temp: Vec<T> = self.a[base_a as usize..(base_a + length_a) as usize].to_vec();
        let mut dest = base_a;
        let mut cursor_temp: isize = 0;
        let mut cursor_b = base_b;

        self.a[dest as usize] = self.a[cursor_b as usize].clone();
        dest += 1;
        cursor_b += 1;

        enum End {
            Succeed,
            CopyB,
        }
        let end = 'merge: {
            length_b -= 1;
            if length_b == 0 {
                break 'merge End::Succeed;
            }
            if length_a == 1 {
                break 'merge End::CopyB;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut wins_a: isize = 0;
                let mut wins_b: isize = 0;
                loop {
                    let b = self.a[cursor_b as usize].clone();
                    if self.less(&b, &temp[cursor_temp as usize])? {
                        self.a[dest as usize] = b;
                        dest += 1;
                        cursor_b += 1;
                        wins_b += 1;
                        length_b -= 1;
                        wins_a = 0;
                        if length_b == 0 {
                            break 'merge End::Succeed;
                        }
                        if wins_b >= min_gallop {
                            break;
                        }
                    } else {
                        self.a[dest as usize] = temp[cursor_temp as usize].clone();
                        dest += 1;
                        cursor_temp += 1;
                        wins_a += 1;
                        length_a -= 1;
                        wins_b = 0;
                        if length_a == 1 {
                            break 'merge End::CopyB;
                        }
                        if wins_a >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                let mut first = true;
                while wins_a >= MIN_GALLOP_WINS || wins_b >= MIN_GALLOP_WINS || first {
                    first = false;
                    min_gallop = (min_gallop - 1).max(1);
                    self.min_gallop = min_gallop;

                    let key = self.a[cursor_b as usize].clone();
                    wins_a = self.gallop_right(&temp, In::Temp, &key, cursor_temp, length_a, 0)?;
                    if wins_a > 0 {
                        self.copy_from(&temp, cursor_temp, dest, wins_a);
                        dest += wins_a;
                        cursor_temp += wins_a;
                        length_a -= wins_a;
                        if length_a == 1 {
                            break 'merge End::CopyB;
                        }
                        // Impossible for a consistent comparator, but this
                        // one need not be.
                        if length_a == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.a[dest as usize] = self.a[cursor_b as usize].clone();
                    dest += 1;
                    cursor_b += 1;
                    length_b -= 1;
                    if length_b == 0 {
                        break 'merge End::Succeed;
                    }

                    let key = temp[cursor_temp as usize].clone();
                    wins_b = self.gallop_left(&temp, In::Work, &key, cursor_b, length_b, 0)?;
                    if wins_b > 0 {
                        self.copy_within(cursor_b, dest, wins_b);
                        dest += wins_b;
                        cursor_b += wins_b;
                        length_b -= wins_b;
                        if length_b == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.a[dest as usize] = temp[cursor_temp as usize].clone();
                    dest += 1;
                    cursor_temp += 1;
                    length_a -= 1;
                    if length_a == 1 {
                        break 'merge End::CopyB;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        match end {
            End::Succeed => {
                if length_a > 0 {
                    self.copy_from(&temp, cursor_temp, dest, length_a);
                }
            }
            End::CopyB => {
                self.copy_within(cursor_b, dest, length_b);
                self.a[(dest + length_b) as usize] = temp[cursor_temp as usize].clone();
            }
        }
        Ok(())
    }

    /// `MergeHigh`: merge backwards, with run B (the shorter) copied out.
    fn merge_high(
        &mut self,
        base_a: isize,
        length_a_arg: isize,
        base_b: isize,
        length_b_arg: isize,
    ) -> Result<(), E> {
        let mut length_a = length_a_arg;
        let mut length_b = length_b_arg;
        let temp: Vec<T> = self.a[base_b as usize..(base_b + length_b) as usize].to_vec();
        let mut dest = base_b + length_b - 1;
        let mut cursor_temp = length_b - 1;
        let mut cursor_a = base_a + length_a - 1;

        self.a[dest as usize] = self.a[cursor_a as usize].clone();
        dest -= 1;
        cursor_a -= 1;

        enum End {
            Succeed,
            CopyA,
        }
        let end = 'merge: {
            length_a -= 1;
            if length_a == 0 {
                break 'merge End::Succeed;
            }
            if length_b == 1 {
                break 'merge End::CopyA;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut wins_a: isize = 0;
                let mut wins_b: isize = 0;
                loop {
                    let a = self.a[cursor_a as usize].clone();
                    if self.less(&temp[cursor_temp as usize], &a)? {
                        self.a[dest as usize] = a;
                        dest -= 1;
                        cursor_a -= 1;
                        wins_a += 1;
                        length_a -= 1;
                        wins_b = 0;
                        if length_a == 0 {
                            break 'merge End::Succeed;
                        }
                        if wins_a >= min_gallop {
                            break;
                        }
                    } else {
                        self.a[dest as usize] = temp[cursor_temp as usize].clone();
                        dest -= 1;
                        cursor_temp -= 1;
                        wins_b += 1;
                        length_b -= 1;
                        wins_a = 0;
                        if length_b == 1 {
                            break 'merge End::CopyA;
                        }
                        if wins_b >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                let mut first = true;
                while wins_a >= MIN_GALLOP_WINS || wins_b >= MIN_GALLOP_WINS || first {
                    first = false;
                    min_gallop = (min_gallop - 1).max(1);
                    self.min_gallop = min_gallop;

                    let key = temp[cursor_temp as usize].clone();
                    let k =
                        self.gallop_right(&temp, In::Work, &key, base_a, length_a, length_a - 1)?;
                    wins_a = length_a - k;
                    if wins_a > 0 {
                        dest -= wins_a;
                        cursor_a -= wins_a;
                        self.copy_within(cursor_a + 1, dest + 1, wins_a);
                        length_a -= wins_a;
                        if length_a == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.a[dest as usize] = temp[cursor_temp as usize].clone();
                    dest -= 1;
                    cursor_temp -= 1;
                    length_b -= 1;
                    if length_b == 1 {
                        break 'merge End::CopyA;
                    }

                    let key = self.a[cursor_a as usize].clone();
                    let k = self.gallop_left(&temp, In::Temp, &key, 0, length_b, length_b - 1)?;
                    wins_b = length_b - k;
                    if wins_b > 0 {
                        dest -= wins_b;
                        cursor_temp -= wins_b;
                        self.copy_from(&temp, cursor_temp + 1, dest + 1, wins_b);
                        length_b -= wins_b;
                        if length_b == 1 {
                            break 'merge End::CopyA;
                        }
                        // Impossible for a consistent comparator, but this
                        // one need not be.
                        if length_b == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.a[dest as usize] = self.a[cursor_a as usize].clone();
                    dest -= 1;
                    cursor_a -= 1;
                    length_a -= 1;
                    if length_a == 0 {
                        break 'merge End::Succeed;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        match end {
            End::Succeed => {
                if length_b > 0 {
                    self.copy_from(&temp, 0, dest - (length_b - 1), length_b);
                }
            }
            End::CopyA => {
                dest -= length_a;
                cursor_a -= length_a;
                self.copy_within(cursor_a + 1, dest + 1, length_a);
                self.a[dest as usize] = temp[cursor_temp as usize].clone();
            }
        }
        Ok(())
    }
}
