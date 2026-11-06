use artisan_api::{CollectionRange, CollectionRef};

pub fn valid_page<T>(source: &CollectionRef, page: &CollectionRange<T>, index: u64) -> bool {
    page.source == source.source
        && page.total == source.count
        && index >= page.start
        && index < page.end
}

pub fn page_record<'a, T>(
    source: &CollectionRef,
    page: &'a CollectionRange<T>,
    index: u64,
) -> Option<&'a T> {
    if !valid_page(source, page, index) {
        return None;
    }
    usize::try_from(index - page.start)
        .ok()
        .and_then(|offset| page.records.get(offset))
}
