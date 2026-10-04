//! `functional-red-black-tree` 1.0.1's insert and remove, as the reference's `getTop` uses them:
//! with their search paths and rebalancing, the pre-check against the end key, and the eviction of
//! the end, so that the pairs the tree compares, and so the comparisons that throw, are the
//! reference's.
//!
//! The reference's tree is persistent and copies the path of every change; this one changes an
//! arena in place, which gives the same shapes because `getTop` drops the old tree at once. A
//! removal that does not find its key (a comparator that is not a total order) removes nothing.

use std::cmp::Ordering::{self, Equal, Greater, Less};

use crate::errors::ApiError;

/// The path of a search from the root, and the last comparison it made.
type Trail = (Vec<usize>, Ordering);

/// The comparator of a tree: the order of its first key against its second.
pub type Cmp<'a, K> = &'a mut dyn FnMut(&K, &K) -> Result<Ordering, ApiError>;

/// The first `count` of `items` in the order of `compare`, which receives each item with its
/// iteration index; the first error of `compare` is the answer.
pub fn top<T>(
    count: usize,
    items: impl Iterator<Item = T>,
    mut compare: impl FnMut(&(usize, T), &(usize, T)) -> Result<Ordering, ApiError>,
) -> Result<Vec<T>, ApiError> {
    let mut tree = RbTree::default();
    for (index, item) in items.enumerate().take_while(|_| count > 0) {
        let key = (index, item);
        let end = tree.last().and_then(|id| tree.key(id));
        if tree.len < count || end.map(|end| compare(&key, end)).transpose()? == Some(Less) {
            tree.insert(key, &mut compare)?;
        }
        if tree.len > count {
            tree.remove_last(&mut compare)?;
        }
    }
    Ok(tree.into_keys().into_iter().map(|(_, item)| item).collect())
}

/// A node's colour and links (left, right), by arena id.
#[derive(Clone, Copy, Default)]
struct Node {
    red: bool,
    kids: [Option<usize>; 2],
}

/// The reference's red-black tree: keys in the order of the comparator that each call receives.
pub struct RbTree<K> {
    nodes: Vec<Node>,
    keys: Vec<Option<K>>,
    root: Option<usize>,
    len: usize,
}

impl<K> Default for RbTree<K> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            keys: Vec::new(),
            root: None,
            len: 0,
        }
    }
}

impl<K> RbTree<K> {
    /// The keys in order (`tree.keys`).
    pub fn into_keys(mut self) -> Vec<K> {
        let (mut keys, mut path, mut next) = (Vec::new(), Vec::new(), self.root);
        loop {
            while let Some(id) = next {
                path.push(id);
                next = self.child(id, false);
            }
            let Some(id) = path.pop() else { return keys };
            keys.extend(self.keys.get_mut(id).and_then(Option::take));
            next = self.child(id, true);
        }
    }

    /// `tree.insert(key)`: a key goes left of the nodes it does not exceed.
    pub fn insert(&mut self, key: K, cmp: Cmp<'_, K>) -> Result<(), ApiError> {
        let (mut path, order) = self.walk(&key, cmp, false)?;
        let id = self.nodes.len();
        self.nodes.push(Node::default());
        self.paint(&[id], true);
        self.keys.push(Some(key));
        self.len += 1;
        match path.last() {
            Some(&parent) => self.link(parent, order == Greater, Some(id)),
            None => self.root = Some(id),
        }
        path.push(id);
        while let [.., pp, p, n] = *path.as_slice() {
            if !self.node(p).red || !self.node(n).red {
                break;
            }
            let p_right = self.child(pp, true) == Some(p);
            let n_right = self.child(p, true) == Some(n);
            if let Some(uncle) = self.child(pp, !p_right).filter(|&y| self.node(y).red) {
                self.paint(&[p, uncle], false);
                self.paint(&[pp], true);
                path.truncate(path.len().saturating_sub(2));
                continue;
            }
            if p_right != n_right {
                self.rotate(p, n_right, Some(pp));
            }
            let top = self.rotate(pp, p_right, path.iter().rev().nth(3).copied());
            self.paint(&[pp], true);
            self.paint(&[top], false);
            break;
        }
        if let Some(root) = self.root {
            self.paint(&[root], false);
        }
        Ok(())
    }

    /// `tree.remove(key)`: nothing when the search of the key ends without the node.
    pub fn remove(&mut self, key: &K, cmp: Cmp<'_, K>) -> Result<(), ApiError> {
        self.walk(key, cmp, true).map(|trail| self.detach(trail))
    }

    /// `tree.remove(tree.end.key)`.
    fn remove_last(&mut self, cmp: Cmp<'_, K>) -> Result<(), ApiError> {
        let Some(end) = self.last().and_then(|id| self.key(id)) else {
            return Ok(());
        };
        self.walk(end, cmp, true).map(|trail| self.detach(trail))
    }

    /// The path from the root that a search for `key` takes, and the last comparison; the search
    /// stops at a node that equals the key when `stop` is set.
    fn walk(&self, key: &K, cmp: Cmp<'_, K>, stop: bool) -> Result<Trail, ApiError> {
        let (mut path, mut order, mut next) = (Vec::new(), Less, self.root);
        while let Some(id) = next {
            order = self.key(id).map_or(Ok(Equal), |node| cmp(key, node))?;
            path.push(id);
            next = if stop && order == Equal {
                None
            } else {
                self.child(id, order == Greater)
            };
        }
        Ok((path, order))
    }

    fn node(&self, id: usize) -> Node {
        self.nodes.get(id).copied().unwrap_or_default()
    }

    fn key(&self, id: usize) -> Option<&K> {
        self.keys.get(id)?.as_ref()
    }

    fn last(&self) -> Option<usize> {
        let mut id = self.root?;
        while let Some(right) = self.child(id, true) {
            id = right;
        }
        Some(id)
    }

    fn child(&self, id: usize, right: bool) -> Option<usize> {
        let node = self.node(id);
        node.kids.get(usize::from(right)).copied().flatten()
    }

    fn edit(&mut self, id: usize, edit: impl FnMut(&mut Node)) {
        self.nodes.get_mut(id).into_iter().for_each(edit);
    }

    fn link(&mut self, id: usize, right: bool, to: Option<usize>) {
        self.edit(id, |node| {
            node.kids
                .get_mut(usize::from(right))
                .into_iter()
                .for_each(|slot| *slot = to);
        });
    }

    fn paint(&mut self, ids: &[usize], red: bool) {
        ids.iter()
            .for_each(|&id| self.edit(id, |node| node.red = red));
    }

    /// `parent`'s link to `old` (its left one, or else its right one) goes to `new`.
    fn replace(&mut self, parent: usize, old: usize, new: Option<usize>) {
        self.link(parent, self.child(parent, true) == Some(old), new);
    }

    /// `old` under `above` (or at the root) is now `new`.
    fn reroot(&mut self, above: Option<usize>, old: usize, new: usize) {
        match above {
            Some(parent) => self.replace(parent, old, Some(new)),
            None => self.root = Some(new),
        }
    }

    /// A rotation that lifts the `right` child of `x` (under `above`), which is returned.
    fn rotate(&mut self, x: usize, right: bool, above: Option<usize>) -> usize {
        let y = self.child(x, right).unwrap_or(x);
        self.link(x, right, self.child(y, !right));
        self.link(y, !right, Some(x));
        self.reroot(above, x, y);
        y
    }

    /// The reference's iterator `remove`, for the path down to the node to remove: a node with two
    /// children takes the key of its predecessor, which is the node that goes.
    fn detach(&mut self, (mut path, order): Trail) {
        let (Some(&found), Equal) = (path.last(), order) else {
            return;
        };
        let mut n = found;
        if let [Some(mut c), Some(_)] = self.node(n).kids {
            while let Some(right) = self.child(c, true) {
                path.push(c);
                c = right;
            }
            path.push(c);
            self.keys.swap(found, c);
            n = c;
        }
        self.len = self.len.saturating_sub(1);
        let node = self.node(n);
        let parent = path.iter().rev().nth(1).copied();
        if let Some(c) = node
            .kids
            .iter()
            .flatten()
            .next()
            .copied()
            .filter(|_| !node.red)
        {
            self.keys.swap(n, c);
            let kids = self.node(c).kids;
            self.edit(n, |node| *node = Node { red: false, kids });
            n = c;
        } else if let Some(parent) = parent {
            if !node.red {
                path.pop();
                self.double_black(n, path);
            }
            self.replace(parent, n, None);
        } else {
            self.root = None;
        }
        drop(self.keys.get_mut(n).and_then(Option::take));
    }

    /// `fixDoubleBlack`: the removed black leaf `n`, below `path`, leaves its path one black short;
    /// a red child of the sibling, or the sibling's colour, or a rotation makes it up.
    fn double_black(&mut self, mut n: usize, mut path: Vec<usize>) {
        while let Some(&p) = path.last() {
            let right = self.child(p, true) == Some(n);
            let Some(s) = self.child(p, !right) else {
                return;
            };
            let red = |tree: &Self, id: Option<usize>| id.filter(|&id| tree.node(id).red);
            let far = red(self, self.child(s, !right));
            let near = red(self, self.child(s, right));
            let plain = far.is_none() && near.is_none();
            if plain && !self.node(s).red {
                self.paint(&[s], true);
                if self.node(p).red {
                    return self.paint(&[p], false);
                }
                n = p;
                path.pop();
                continue;
            }
            if let (None, Some(_)) = (far, near) {
                self.rotate(s, right, Some(p));
            }
            let top = self.rotate(p, !right, path.iter().rev().nth(1).copied());
            self.paint(&[top], self.node(p).red);
            if plain {
                self.paint(&[p], true);
                path.pop();
                path.extend([top, p]);
                continue;
            }
            return self.paint(&[n, p, far.unwrap_or(s)], false);
        }
        self.paint(&[n], false);
    }
}
