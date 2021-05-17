use std::slice;

/// A sparse set used for representing ordered NFA states.
///
/// This supports constant time addition and membership testing. Clearing an
/// entire set can also be done in constant time. Iteration yields elements
/// in the order in which they were inserted.
///
/// The data structure is based on: http://research.swtch.com/sparse
/// Note though that we don't actually use uninitialized memory. We generally
/// reuse sparse sets, so the initial allocation cost is bareable. However, its
/// other properties listed above are extremely useful.
#[derive(Clone, Debug)]
pub struct SparseSet {
    /// Dense contains the instruction pointers in the order in which they
    /// were inserted.
    dense: Vec<usize>,
    /// Sparse maps instruction pointers to their location in dense.
    ///
    /// An instruction pointer is in the set if and only if
    /// sparse[ip] < dense.len() && ip == dense[sparse[ip]].
    sparse: Box<[usize]>,
}

impl SparseSet {
    pub fn new(size: usize) -> SparseSet {
        SparseSet {
            dense: Vec::with_capacity(size),
            sparse: vec![0; size].into_boxed_slice(),
        }
    }

    pub fn len(&self) -> usize {
        self.dense.len()
    }

    pub fn insert(&mut self, value: usize) {
        let i = self.len();
        assert!(i < self.dense.capacity());
        self.dense.push(value);
        self.sparse[value] = i;
    }

    pub fn contains(&self, value: usize) -> bool {
        let i = self.sparse[value];
        self.dense.get(i) == Some(&value)
    }

    pub fn clear(&mut self) {
        self.dense.clear();
    }
}

impl<'a> IntoIterator for &'a SparseSet {
    type Item = &'a usize;
    type IntoIter = slice::Iter<'a, usize>;
    fn into_iter(self) -> Self::IntoIter {
        self.dense.iter()
    }
}
