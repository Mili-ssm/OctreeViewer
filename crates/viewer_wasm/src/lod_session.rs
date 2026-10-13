use crate::LodDataset;

/// A reader can only exist alongside the dataset whose blocks it supplies.
pub(crate) enum LodSession<R> {
    External(LodDataset),
    Format { dataset: LodDataset, reader: R },
}

impl<R> LodSession<R> {
    pub(crate) fn dataset(&self) -> &LodDataset {
        match self {
            Self::External(dataset) | Self::Format { dataset, .. } => dataset,
        }
    }

    pub(crate) fn dataset_mut(&mut self) -> &mut LodDataset {
        self.parts_mut().0
    }

    pub(crate) fn reader(&self) -> Option<&R> {
        match self {
            Self::External(_) => None,
            Self::Format { reader, .. } => Some(reader),
        }
    }

    pub(crate) fn parts_mut(&mut self) -> (&mut LodDataset, Option<&mut R>) {
        match self {
            Self::External(dataset) => (dataset, None),
            Self::Format { dataset, reader } => (dataset, Some(reader)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};
    use viewer_core::{
        BlockId, ColorRgb, NodeId, Point, PointBatch, PointBlock, SpatialBounds, SpatialHierarchy,
        SpatialNode,
    };

    fn dataset() -> LodDataset {
        let node = SpatialNode::new(
            NodeId::new(0),
            None,
            0,
            SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
            1,
            0.0,
            vec![BlockId::new(7)],
        )
        .unwrap();
        LodDataset::new(
            [0.0; 3],
            SpatialHierarchy::new(vec![node], vec![], Some(1)).unwrap(),
            4096,
        )
        .unwrap()
    }

    struct Reader(Rc<Cell<usize>>);
    impl Drop for Reader {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn switching_sessions_releases_reader_and_external_blocks_remain_insertable() {
        let dropped = Rc::new(Cell::new(0));
        let mut session = Some(LodSession::External(dataset()));
        assert!(session.as_ref().unwrap().reader().is_none());
        session = Some(LodSession::Format {
            dataset: dataset(),
            reader: Reader(Rc::clone(&dropped)),
        });
        {
            let (dataset, reader) = session.as_mut().unwrap().parts_mut();
            assert!(reader.is_some());
            assert!(dataset.references_block(BlockId::new(7)));
        }
        assert_eq!(dropped.get(), 0);
        session = Some(LodSession::External(dataset()));
        assert_eq!(dropped.get(), 1);
        let external = session.as_mut().unwrap();
        let block = PointBlock::new(
            BlockId::new(7),
            PointBatch::new(vec![Point::new([0.5; 3], ColorRgb::new(10, 20, 30))]),
            vec![],
        );
        assert!(external.dataset_mut().insert_block(block).unwrap() > 0);
        assert!(external.dataset().references_block(BlockId::new(7)));
        assert!(external.parts_mut().1.is_none());
        session = Some(LodSession::Format {
            dataset: dataset(),
            reader: Reader(Rc::clone(&dropped)),
        });
        drop(session.take());
        assert!(session.is_none());
        assert_eq!(dropped.get(), 2);
    }
}
