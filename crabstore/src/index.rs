
use core::fmt;
use std::collections::BTreeMap;
use pyo3::{
    prelude::*,
    types::{PyList, PyTuple},
};

use crate::rid::RID;

#[derive(Clone, Debug, Default)]
#[pyclass(subclass)]
//change to BTreeMap when we need to implement ranges
pub struct Index {
    indices: Vec<Option<BTreeMap<i64, Vec<RID>>>>,
}
impl fmt::Display for Index {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (i, v) in self.indices.iter().enumerate() {
            write!(f, "Index on Column {}:\n", i);
            match v {
                Some(v) => {
                    for (key, value) in v.iter() {
                        write!(f, "Key: {} | Value: {:?}\n", key, value)?;
                    }
                }
                None => {
                    write!(f, "None\n");
                }
            }
        }

        Ok(())
    }
}
impl Index {
    pub fn new(key_index: usize, num_columns: usize) -> Self {
        let mut indices = Vec::with_capacity(num_columns);
        indices.resize_with(num_columns, Default::default);
        indices[key_index] = Some(BTreeMap::new());

        Index { indices }
    }

    pub fn update_index(&mut self, column_number: usize, value: i64, rid: RID) {
        if let Some(ref mut index) = self.indices[column_number] {
            if let Some(ref mut rids) = index.get_mut(&value) {
                rids.push(rid);
            } else {
                index.insert(value, vec![rid]);
            }
        }
    }
    pub fn get_from_index(&self, column_number: usize, value: i64) -> Option<Vec<RID>> {
        self.indices[column_number]
            .as_ref()
            .map(|map| match map.get(&value) {
                None => Vec::new(),
                Some(rids) => rids.clone(),
            })
    }
    pub fn range_from_index(&self, column_number: usize, begin: i64, end: i64) -> Option<Vec<RID>> {
        self.indices[column_number].as_ref().map(|map| {
            map.range(begin..end)
                .flat_map(|item| item.1.clone())
                .collect::<Vec<RID>>()
        })
    }
    pub fn create_index(&mut self, column_number: usize) {
        self.indices[column_number] = Some(BTreeMap::new());
    }

    pub fn drop_index(&mut self, column_number: usize) {
        self.indices[column_number] = None;
    }
}