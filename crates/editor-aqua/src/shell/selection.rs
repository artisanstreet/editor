use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Default)]
pub struct SelectionAllocator(Rc<Cell<u64>>);

impl SelectionAllocator {
    fn next(&self) -> u64 {
        let id = self
            .0
            .get()
            .checked_add(1)
            .expect("selection identity exhausted");
        self.0.set(id);
        id
    }

    pub fn workspace(&self) -> SelectionScope {
        let key = self.next();
        let pair = || {
            (
                SelectionDocumentId(self.next()),
                SelectionSourceId(self.next()),
            )
        };
        let (source_document, source) = pair();
        let (command_document, command) = pair();
        let (output_document, output) = pair();
        let (detail_document, detail) = pair();
        SelectionScope {
            key,
            inspector: crate::inspector::SourceIds {
                source_document,
                command_document,
                output_document,
                detail_document,
                source,
                command,
                output,
                detail,
            },
            conversation: SelectionDocumentId(self.next()),
            question: (
                SelectionDocumentId(self.next()),
                SelectionSourceId(self.next()),
            ),
            question_options: (
                SelectionDocumentId(self.next()),
                SelectionSourceId(self.next()),
            ),
            question_option_detail: (
                SelectionDocumentId(self.next()),
                SelectionSourceId(self.next()),
            ),
            allocator: self.clone(),
        }
    }
}

pub struct SelectionScope {
    pub key: u64,
    pub inspector: crate::inspector::SourceIds,
    pub conversation: SelectionDocumentId,
    pub question: (SelectionDocumentId, SelectionSourceId),
    pub question_options: (SelectionDocumentId, SelectionSourceId),
    pub question_option_detail: (SelectionDocumentId, SelectionSourceId),
    allocator: SelectionAllocator,
}

impl SelectionScope {
    pub fn source_allocator(&self) -> Rc<dyn Fn() -> SelectionSourceId> {
        let allocator = self.allocator.clone();
        Rc::new(move || SelectionSourceId(allocator.next()))
    }

    pub fn next_source(&self) -> SelectionSourceId {
        SelectionSourceId(self.allocator.next())
    }

    pub fn next_document(&self) -> SelectionDocumentId {
        SelectionDocumentId(self.allocator.next())
    }
}
