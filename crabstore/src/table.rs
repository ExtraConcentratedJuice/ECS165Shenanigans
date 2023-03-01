use std::{mem::size_of, path::Path};

use crate::{
    disk_manager::DiskManager,
    page::PhysicalPage,
    rid::{BaseRID, TailRID, RID},
};
use crate::{index::Index, RID_INVALID};
use crate::{
    page::{Page, PageRange},
    page_directory::PageDirectory,
};
use crate::{
    Record, METADATA_INDIRECTION, METADATA_RID, METADATA_SCHEMA_ENCODING, NUM_METADATA_COLUMNS,
};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};
use rayon::{prelude::*, result};
use rkyv::{
    ser::{serializers::BufferSerializer, Serializer},
    Archive, Deserialize, Serialize,
};

#[derive(Archive, Deserialize, Serialize, Clone, Debug)]
pub struct TableHeaderPage {
    num_columns: usize,
    primary_key_index: usize,
    indexed_columns: usize,
    next_free_page: usize,
    next_rid: usize,
    next_tid: usize,
}

#[pyclass(subclass)]
pub struct Table {
    name: String,
    num_columns: usize,
    primary_key_index: usize,
    index: Index,
    next_rid: BaseRID,
    ranges: Vec<PageRange>,
    page_dir: PageDirectory,
    disk: DiskManager,
}

impl Table {
    pub fn load(name: &str, db_file: &Path, pd_file: &Path) -> Self {
        let disk = DiskManager::new(db_file).expect("Failed to open table file");

        let mut page = PhysicalPage::default();

        disk.read_page(0, &mut page.page);

        let header = unsafe {
            rkyv::from_bytes_unchecked::<TableHeaderPage>(
                &page.page[0..size_of::<<TableHeaderPage as Archive>::Archived>()],
            )
            .expect("Failed to deserialize table header")
        };

        let index = Index::new(header.primary_key_index, header.num_columns);

        let page_dir = PageDirectory::load(pd_file);

        Table {
            name: name.into(),
            num_columns: header.num_columns,
            primary_key_index: header.primary_key_index,
            index,
            next_rid: 0.into(),
            ranges: Vec::new(),
            page_dir,
            disk,
        }
    }

    pub fn persist(&mut self) {
        let header = TableHeaderPage {
            num_columns: self.num_columns,
            primary_key_index: self.primary_key_index,
            next_rid: 100,
            next_tid: 250,
            next_free_page: self.disk.free_page_pointer(),
            indexed_columns: self.index.index_meta_to_bit_vector(),
        };

        let mut page = PhysicalPage::default();
        let mut serializer = BufferSerializer::new(&mut page.page);

        serializer
            .serialize_value(&header)
            .expect("Unable to serialize table header");

        self.disk.write_page(0, &page);
        self.disk.flush();
        self.page_dir.persist();
    }

    fn get_page_range(&self, range_number: usize) -> &PageRange {
        &self.ranges[range_number]
    }

    fn get_page_range_mut(&mut self, range_number: usize) -> &mut PageRange {
        &mut self.ranges[range_number]
    }
    fn get_base_page(&self, rid: &BaseRID) -> Option<&Page> {
        self.ranges[rid.page_range()].get_base_page(rid)
    }
    fn get_base_page_mut(&mut self, rid: &BaseRID) -> Option<&mut Page> {
        self.ranges[rid.page_range()].get_base_page_mut(rid)
    }
    fn get_tail_page(&self, rid: &TailRID) -> Option<&Page> {
        self.ranges[rid.page_range()].get_tail_page(rid)
    }
    fn get_tail_page_mut(&mut self, rid: &TailRID) -> Option<&mut Page> {
        self.ranges[rid.page_range()].get_tail_page_mut(rid)
    }
    fn find_row(&self, column_index: usize, value: i64) -> Option<BaseRID> {
        match self.index.get_from_index(column_index, value) {
            Some(vals) => vals
                .iter()
                .find(|x| self.get_base_page(x).unwrap().get_slot(METADATA_RID, *x) != RID_INVALID)
                .copied(),
            None => {
                let rid: BaseRID = 0.into();

                while rid.raw() < self.next_rid.raw() {
                    let base_page = self.get_page_range(rid.page_range()).get_base_page(&rid);
                    let (latest_page, latest_rid) = self.get_latest_page_rid(&rid);

                    if base_page.unwrap().get_column(METADATA_RID).slot(rid.slot()) != RID_INVALID
                        && latest_page
                            .get_column(NUM_METADATA_COLUMNS + column_index)
                            .slot(latest_rid.slot())
                            == value
                    {
                        return Some(rid);
                    }
                }
                None
            }
        }
    }
    fn find_rows(&self, column_index: usize, value: i64) -> Vec<BaseRID> {
        match self.index.get_from_index(column_index, value) {
            Some(vals) => vals
                .into_iter()
                .filter(|x| self.get_base_page(x).unwrap().get_slot(METADATA_RID, x) != RID_INVALID)
                .collect(),
            None => {
                let mut rid: BaseRID = 0.into();
                let mut rids = Vec::new();

                while rid.raw() < self.next_rid.raw() {
                    let base_page = self.get_page_range(rid.page_range()).get_base_page(&rid);

                    if base_page.unwrap().get_column(METADATA_RID).slot(rid.slot()) == RID_INVALID {
                        rid = rid.next();
                        continue;
                    }

                    if self.is_latest(&rid) {
                        if base_page
                            .unwrap()
                            .get_column(NUM_METADATA_COLUMNS + column_index)
                            .slot(rid.slot())
                            == value
                        {
                            rids.push(rid);
                        }
                    } else {
                        let latest_rid = self.find_latest(&rid);

                        if self
                            .get_tail_page(&latest_rid)
                            .unwrap()
                            .get_column(NUM_METADATA_COLUMNS + column_index)
                            .slot(latest_rid.slot())
                            == value
                        {
                            rids.push(rid);
                        }
                    }

                    rid = rid.next();
                }

                rids
            }
        }
    }
    fn find_rows_range(&self, column_index: usize, begin: i64, end: i64) -> Vec<BaseRID> {
        match self.index.range_from_index(column_index, begin, end) {
            Some(vals) => vals,
            None => {
                let mut rids: Vec<BaseRID> = Vec::new();
                let mut rid: BaseRID = 0.into();
                while rid.raw() < self.next_rid.raw() {
                    let page = self.get_page_range(rid.page_range()).get_base_page(&rid);

                    let key = page
                        .unwrap()
                        .get_column(NUM_METADATA_COLUMNS + self.primary_key_index)
                        .slot(rid.slot());

                    if key >= begin && key <= end {
                        rids.push(rid);
                    }

                    rid = rid.next();
                }
                rids
            }
        }
    }
    pub fn is_latest(&self, rid: &BaseRID) -> bool {
        self.get_base_page(rid)
            .unwrap()
            .get_column(METADATA_SCHEMA_ENCODING)
            .slot(rid.slot())
            == 0
    }
    pub fn find_latest(&self, rid: &BaseRID) -> TailRID {
        let page = self.get_base_page(rid);

        TailRID::from(
            page.unwrap()
                .get_column(METADATA_INDIRECTION)
                .slot(rid.slot()),
        )
    }
    pub fn get_latest_page_rid(&self, rid: &BaseRID) -> (&Page, Box<dyn RID>) {
        match self.is_latest(rid) {
            true => (self.get_base_page(rid).unwrap(), Box::new(*rid)),
            false => {
                let newrid = Box::new(self.find_latest(rid));
                (self.get_tail_page(&newrid).unwrap(), newrid)
            }
        }
    }
    pub fn get_latest_page_rid_mut(&mut self, rid: &BaseRID) -> (&mut Page, Box<dyn RID>) {
        match self.is_latest(rid) {
            true => (self.get_base_page_mut(rid).unwrap(), Box::new(*rid)),
            false => {
                let newrid = Box::new(self.find_latest(rid));
                (self.get_tail_page_mut(&newrid).unwrap(), newrid)
            }
        }
    }
}

#[pymethods]
impl Table {
    #[getter]
    fn num_columns(&self) -> usize {
        self.num_columns
    }

    #[new]
    pub fn new(
        name: String,
        num_columns: usize,
        key_index: usize,
        db_file: String,
        pd_file: String,
    ) -> Table {
        let page_dir = PageDirectory::new(Path::new(&pd_file));
        let disk = DiskManager::new(Path::new(&db_file)).unwrap();

        Table {
            name,
            num_columns,
            primary_key_index: key_index,
            index: Index::new(key_index, num_columns),
            next_rid: 0.into(),
            ranges: Vec::new(),
            page_dir,
            disk,
        }
    }
    pub fn sum(&self, start_range: i64, end_range: i64, column_index: usize) -> i64 {
        let vec: Vec<i64> = self
            .find_rows_range(column_index, start_range, end_range)
            .iter()
            .map(|rid| {
                let (page, newrid) = self.get_latest_page_rid(rid);
                page.get_column(NUM_METADATA_COLUMNS + column_index)
                    .slot(newrid.slot())
            })
            .collect::<Vec<i64>>();
        vec.iter().sum()
    }

    pub fn select(&self, search_value: i64, column_index: usize, columns: &PyList) -> Py<PyList> {
        let included_columns: Vec<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_i, x)| x.extract::<i64>().unwrap() != 0)
            .map(|(i, _x)| i)
            .collect();

        let vals: Vec<BaseRID> = self.find_rows(column_index, search_value);

        Python::with_gil(|py| -> Py<PyList> {
            let selected_records: Py<PyList> = PyList::empty(py).into();
            let mut page: Option<&Page>;

            for rid in vals {
                let result_cols = PyList::empty(py);

                if self.is_latest(&rid) {
                    page = self.get_base_page(&rid);
                    for i in included_columns.iter() {
                        result_cols.append(
                            page.unwrap()
                                .get_column(NUM_METADATA_COLUMNS + i)
                                .slot(rid.slot()),
                        );
                    }
                } else {
                    let rid: TailRID = self.find_latest(&rid);

                    page = self.get_tail_page(&rid);

                    for i in included_columns.iter() {
                        result_cols.append(
                            page.unwrap()
                                .get_column(NUM_METADATA_COLUMNS + i)
                                .slot(rid.slot()),
                        );
                    }
                }

                let record = PyCell::new(
                    py,
                    Record::new(
                        page.unwrap().get_column(METADATA_RID).slot(rid.slot()) as u64,
                        page.unwrap()
                            .get_column(METADATA_INDIRECTION)
                            .slot(rid.slot()) as u64,
                        page.unwrap()
                            .get_column(METADATA_SCHEMA_ENCODING)
                            .slot(rid.slot()) as u64,
                        result_cols.into(),
                    ),
                )
                .unwrap();

                selected_records.as_ref(py).append(record);
            }
            selected_records
        })
    }

    pub fn update(&mut self, search_value: i64, values: &PyTuple) -> bool {
        let vals: Vec<Option<i64>> = values
            .iter()
            .map(|val| val.extract::<Option<i64>>().unwrap())
            .collect::<Vec<Option<i64>>>();

        let row = self.find_row(self.primary_key_index, search_value);

        if row.is_none() {
            return false;
        }

        let row = row.unwrap();
        let old_latest_rid = self.find_latest(&row);
        let old_schema_encoding = self
            .get_base_page(&row)
            .unwrap()
            .get_slot(METADATA_SCHEMA_ENCODING, &row);

        let mut schema_encoding: i64 = 0;

        for (i, v) in vals.iter().enumerate() {
            if !v.is_none() {
                schema_encoding |= 1 << i;
                self.index.update_index(i, v.unwrap(), row);
                if old_schema_encoding == 0 {
                    self.index.remove_index(
                        i,
                        self.get_base_page(&row)
                            .unwrap()
                            .get_slot(NUM_METADATA_COLUMNS + i, &row),
                        row,
                    );
                } else {
                    self.index.remove_index(
                        i,
                        self.get_tail_page(&old_latest_rid)
                            .unwrap()
                            .get_slot(NUM_METADATA_COLUMNS + i, &old_latest_rid),
                        row,
                    );
                }
            }
        }

        //print!("Update called\n");
        let tail_rid = self
            .get_page_range_mut(row.page_range())
            .append_update_record(&row, &vals);

        self.get_base_page_mut(&row)
            .unwrap()
            .get_column_mut(METADATA_INDIRECTION)
            .write_slot(row.slot(), tail_rid.raw());

        self.get_base_page_mut(&row)
            .unwrap()
            .get_column_mut(METADATA_SCHEMA_ENCODING)
            .write_slot(row.slot(), schema_encoding);

        true
    }

    pub fn delete(&mut self, key: i64) -> bool {
        let row = self.find_row(self.primary_key_index, key);

        if row.is_none() {
            return false;
        }

        let row = row.unwrap();

        let mut next_tail: TailRID = self
            .get_base_page(&row)
            .unwrap()
            .get_slot(METADATA_INDIRECTION, &row)
            .into();

        while !next_tail.raw() == RID_INVALID && next_tail.raw() != row.raw() {
            let next: TailRID = self
                .get_tail_page(&next_tail)
                .unwrap()
                .get_slot(METADATA_INDIRECTION, &next_tail)
                .into();

            self.get_tail_page_mut(&next_tail).unwrap().write_slot(
                METADATA_RID,
                &next_tail,
                RID_INVALID,
            );

            next_tail = next;
        }

        self.get_base_page_mut(&row)
            .unwrap()
            .write_slot(METADATA_RID, &row, RID_INVALID);

        true
    }

    #[args(values = "*")]
    pub fn insert(&mut self, values: &PyTuple) {
        let rid: BaseRID = self.next_rid;
        let page_range = rid.page_range();
        let slot = rid.slot();

        if self.ranges.len() <= page_range {
            self.ranges.push(PageRange::new(self.num_columns));
        }

        let page = self.get_base_page_mut(&rid).unwrap();

        page.get_column_mut(METADATA_RID)
            .write_slot(slot, rid.raw());

        page.get_column_mut(METADATA_SCHEMA_ENCODING)
            .write_slot(slot, 0);

        for (i, val) in values.iter().enumerate() {
            page.get_column_mut(NUM_METADATA_COLUMNS + i)
                .write_slot(slot, val.extract().unwrap())
        }

        self.index.update_index(
            self.primary_key_index,
            values
                .get_item(self.primary_key_index)
                .unwrap()
                .extract()
                .unwrap(),
            rid,
        );

        self.next_rid = rid.next();
    }

    pub fn print(&self) {
        println!("{}", self.name);
        println!("{}", self.num_columns);
        println!("{}", self.primary_key_index);
        println!("{}", self.index);
    }
}
