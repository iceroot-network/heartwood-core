//! Node 18's `Array.prototype.sort`: V8's TimSort (binary insertion below 64 elements; runs,
//! merges and galloping above), so that a comparator that is not a total order gives the
//! reference's order.
//!
//! The sort works on the positions of the items, which it compares through `compare` and moves
//! as V8 moves the elements, then permutes the items once at the end; the items need no `Clone`.

#![allow(
    clippy::indexing_slicing,
    reason = "the sort's positions are always inside the array and the runs it keeps"
)]

use std::cmp::Ordering;

const MIN_GALLOP: isize = 7;

/// Sort `items` in place with `compare` as V8 sorts an array.
pub fn v8_sort<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    let length = items.len();
    if length < 2 {
        return;
    }
    let mut sorter = Sorter {
        work: (0..length).collect(),
        order: |x: usize, y: usize| compare(&items[x], &items[y]) as i8,
        runs: Vec::new(),
        min_gallop: MIN_GALLOP,
    };
    sorter.sort();
    // Move each item to the position the sort gave it, by cycles.
    let mut target = vec![0; length];
    for (position, &item) in sorter.work.iter().enumerate() {
        target[item] = position;
    }
    for at in 0..length {
        while target[at] != at {
            let to = target[at];
            items.swap(at, to);
            target.swap(at, to);
        }
    }
}

struct Sorter<F: FnMut(usize, usize) -> i8> {
    work: Vec<usize>,
    order: F,
    runs: Vec<(isize, isize)>,
    min_gallop: isize,
}

impl<F: FnMut(usize, usize) -> i8> Sorter<F> {
    fn at(&self, index: isize) -> usize {
        self.work[index as usize]
    }

    fn put(&mut self, index: isize, value: usize) {
        self.work[index as usize] = value;
    }

    fn sort(&mut self) {
        let mut remaining = self.work.len() as isize;
        let (mut low, mut min_run) = (0, remaining);
        let mut carry = 0;
        while min_run >= 64 {
            carry |= min_run & 1;
            min_run >>= 1;
        }
        min_run += carry;
        while remaining != 0 {
            let mut run = self.count_and_make_run(low, low + remaining);
            if run < min_run {
                let forced = min_run.min(remaining);
                self.binary_insertion_sort(low, low + run, low + forced);
                run = forced;
            }
            self.runs.push((low, run));
            self.merge_collapse();
            low += run;
            remaining -= run;
        }
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            if n > 0 && self.runs[n - 1].1 < self.runs[n + 1].1 {
                n -= 1;
            }
            self.merge_at(n);
        }
    }

    fn count_and_make_run(&mut self, low_arg: isize, high: isize) -> isize {
        let low = low_arg + 1;
        if low == high {
            return 1;
        }
        let mut run = 2;
        let (newer, older) = (self.at(low), self.at(low - 1));
        let descending = (self.order)(newer, older) < 0;
        let mut previous = self.at(low);
        for index in low + 1..high {
            let current = self.at(index);
            let order = (self.order)(current, previous);
            if (order < 0) != descending {
                break;
            }
            previous = current;
            run += 1;
        }
        if descending {
            self.work[low_arg as usize..(low_arg + run) as usize].reverse();
        }
        run
    }

    fn binary_insertion_sort(&mut self, low: isize, start_arg: isize, high: isize) {
        let mut start = start_arg.max(low + 1);
        while start < high {
            let (mut left, mut right) = (low, start);
            let pivot = self.at(start);
            while left < right {
                let mid = left + ((right - left) >> 1);
                let probe = self.at(mid);
                if (self.order)(pivot, probe) < 0 {
                    right = mid;
                } else {
                    left = mid + 1;
                }
            }
            self.shift(left, left + 1, start - left);
            self.put(left, pivot);
            start += 1;
        }
    }

    fn merge_collapse(&mut self) {
        let established =
            |runs: &[(isize, isize)], n: usize| n < 2 || runs[n - 2].1 > runs[n - 1].1 + runs[n].1;
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            let runs = &self.runs;
            if !established(runs, n + 1) || !established(runs, n) {
                n -= usize::from(n > 0 && runs[n - 1].1 < runs[n + 1].1);
            } else if runs[n].1 > runs[n + 1].1 {
                break;
            }
            self.merge_at(n);
        }
    }

    fn merge_at(&mut self, i: usize) {
        let (mut base_a, mut length_a) = self.runs[i];
        let (base_b, mut length_b) = self.runs[i + 1];
        self.runs[i].1 = length_a + length_b;
        self.runs.remove(i + 1);
        // Where does the first element of B go in A, and the last element of A in B?
        let (order, work) = (&mut self.order, &self.work);
        let key = work[base_b as usize];
        let run = &work[base_a as usize..(base_a + length_a) as usize];
        let k = gallop(order, run, key, true, 0);
        base_a += k;
        length_a -= k;
        if length_a == 0 {
            return;
        }
        let key = work[(base_a + length_a - 1) as usize];
        let run = &work[base_b as usize..(base_b + length_b) as usize];
        length_b = gallop(order, run, key, false, length_b - 1);
        if length_b != 0 {
            self.merge(base_a, length_a, base_b, length_b);
        }
    }

    fn store(&mut self, at: isize, source: &[usize]) {
        self.work[at as usize..at as usize + source.len()].copy_from_slice(source);
    }

    fn shift(&mut self, from: isize, to: isize, count: isize) {
        self.work
            .copy_within(from as usize..(from + count) as usize, to as usize);
    }

    /// V8's MergeLow (when run A is not longer than run B) and MergeHigh: the shorter run is
    /// copied aside (`temp`) and the merge fills from the front (`step` 1) or from the back
    /// (`step` -1) of the two runs. `dest`, `ct` and `cw` are where the next item goes, the next
    /// item of the temporary run and of the run in place (`lt` and `lw` are what is left of them).
    fn merge(&mut self, base_a: isize, length_a: isize, base_b: isize, length_b: isize) {
        let high = length_a > length_b;
        let step = if high { -1 } else { 1 };
        let (mut lt, mut lw) = if high {
            (length_b, length_a)
        } else {
            (length_a, length_b)
        };
        let temp_base = if high { base_b } else { base_a };
        let temp = self.work[temp_base as usize..(temp_base + lt) as usize].to_vec();
        // The lowest index of `len` items that start at `at` and run in the merge's direction.
        let lowest = |at: isize, len: isize| if high { at + 1 - len } else { at };
        let (mut dest, mut ct, mut cw) = if high {
            (base_b + length_b - 1, length_b - 1, base_a + length_a - 1)
        } else {
            (base_a, 0, base_b)
        };
        self.put(dest, self.at(cw));
        (dest, cw, lw) = (dest + step, cw + step, lw - 1);
        // Whether the item in place that is left is the last of the merge (else the temporary's).
        let copy_work = 'merge: {
            if lw == 0 {
                break 'merge false;
            }
            if lt == 1 {
                break 'merge true;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let (mut wins_t, mut wins_w) = (0, 0);
                loop {
                    let (work_item, temp_item) = (self.at(cw), temp[ct as usize]);
                    let (x, y) = if high {
                        (temp_item, work_item)
                    } else {
                        (work_item, temp_item)
                    };
                    if (self.order)(x, y) < 0 {
                        self.put(dest, work_item);
                        (dest, cw, lw) = (dest + step, cw + step, lw - 1);
                        (wins_w, wins_t) = (wins_w + 1, 0);
                        if lw == 0 {
                            break 'merge false;
                        }
                    } else {
                        self.put(dest, temp_item);
                        (dest, ct, lt) = (dest + step, ct + step, lt - 1);
                        (wins_t, wins_w) = (wins_t + 1, 0);
                        if lt == 1 {
                            break 'merge true;
                        }
                    }
                    if wins_w.max(wins_t) >= min_gallop {
                        break;
                    }
                }
                min_gallop += 1;
                let mut first = true;
                while wins_t >= MIN_GALLOP || wins_w >= MIN_GALLOP || first {
                    first = false;
                    min_gallop = (min_gallop - 1).max(1);
                    self.min_gallop = min_gallop;
                    // Low gallops through the temporary run first, high through the run in place.
                    for temp_side in [!high, high] {
                        if temp_side {
                            let (key, hint) = (self.at(cw), if high { lt - 1 } else { 0 });
                            let run = &temp[lowest(ct, lt) as usize..][..lt as usize];
                            let k = gallop(&mut self.order, run, key, !high, hint);
                            wins_t = if high { lt - k } else { k };
                            if wins_t > 0 {
                                let from = lowest(ct, wins_t) as usize;
                                self.store(
                                    lowest(dest, wins_t),
                                    &temp[from..from + wins_t as usize],
                                );
                                (dest, ct, lt) =
                                    (dest + step * wins_t, ct + step * wins_t, lt - wins_t);
                                if lt <= 1 {
                                    break 'merge lt == 1;
                                }
                            }
                            self.put(dest, self.at(cw));
                            (dest, cw, lw) = (dest + step, cw + step, lw - 1);
                            if lw == 0 {
                                break 'merge false;
                            }
                        } else {
                            let (key, hint) = (temp[ct as usize], if high { lw - 1 } else { 0 });
                            let run = &self.work[lowest(cw, lw) as usize..][..lw as usize];
                            let k = gallop(&mut self.order, run, key, high, hint);
                            wins_w = if high { lw - k } else { k };
                            if wins_w > 0 {
                                self.shift(lowest(cw, wins_w), lowest(dest, wins_w), wins_w);
                                (dest, cw, lw) =
                                    (dest + step * wins_w, cw + step * wins_w, lw - wins_w);
                                if lw == 0 {
                                    break 'merge false;
                                }
                            }
                            self.put(dest, key);
                            (dest, ct, lt) = (dest + step, ct + step, lt - 1);
                            if lt == 1 {
                                break 'merge true;
                            }
                        }
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        if copy_work {
            self.shift(lowest(cw, lw), lowest(dest, lw), lw);
            self.put(dest + step * lw, temp[ct as usize]);
        } else if lt > 0 {
            self.store(
                lowest(dest, lt),
                &temp[lowest(ct, lt) as usize..][..lt as usize],
            );
        }
    }
}

/// V8's GallopLeft (`right` false) and GallopRight (`right` true): where `key` goes in the sorted
/// `array`, searching outward from `hint`; the leftmost or the rightmost place among equals.
fn gallop(
    order: &mut impl FnMut(usize, usize) -> i8,
    array: &[usize],
    key: usize,
    right: bool,
    hint: isize,
) -> isize {
    let length = array.len() as isize;
    // Whether `key` goes after the item at `index`.
    let mut after = |index: isize| {
        let item = array[index as usize];
        if right {
            order(key, item) >= 0
        } else {
            order(item, key) < 0
        }
    };
    let (mut last, mut offset) = (0, 1);
    if after(hint) {
        let max = length - hint;
        while offset < max && after(hint + offset) {
            (last, offset) = (offset, (offset << 1) + 1);
        }
        offset = offset.min(max);
        (last, offset) = (last + hint, offset + hint);
    } else {
        let max = hint + 1;
        while offset < max && !after(hint - offset) {
            (last, offset) = (offset, (offset << 1) + 1);
        }
        offset = offset.min(max);
        (last, offset) = (hint - offset, hint - last);
    }
    last += 1;
    while last < offset {
        let mid = last + ((offset - last) >> 1);
        if after(mid) {
            last = mid + 1;
        } else {
            offset = mid;
        }
    }
    offset
}
