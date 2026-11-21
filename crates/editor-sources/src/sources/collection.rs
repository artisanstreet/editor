use artisan_api::{CollectionRange, CollectionRef};

fn valid_page<T>(source: &CollectionRef, page: &CollectionRange<T>, index: u64) -> bool {
    let same_source = page.source == source.source;
    let same_count = page.total == source.count;
    same_source && same_count && (page.start..page.end).contains(&index)
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
