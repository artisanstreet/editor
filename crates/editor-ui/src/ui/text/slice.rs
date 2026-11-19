use std::fmt::Debug;
use std::slice::SliceIndex;

#[track_caller]
pub fn slice<R>(text: &str, range: R) -> &str
where
    R: SliceIndex<str, Output = str> + Clone + Debug,
{
    text.get(range.clone()).unwrap_or_else(|| {
        panic!(
            "invariant: {range:?} lies on char boundaries of a {}-byte string",
            text.len()
        )
    })
}
