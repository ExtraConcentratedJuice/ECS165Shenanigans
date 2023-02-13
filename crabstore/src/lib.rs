use core::fmt;
use pyo3::{
    prelude::*,
    types::{PyList, PyTuple},
};
use std::{borrow::Borrow, cell::RefCell, mem::size_of};
use std::{collections::BTreeMap, collections::HashMap};

const PAGE_SIZE: usize = 4096;
const PAGE_SLOTS: usize = PAGE_SIZE / size_of::<i64>();
const PAGE_RANGE_SIZE: usize = PAGE_SIZE * 16;
const RANGE_PAGE_COUNT: usize = PAGE_RANGE_SIZE / PAGE_SIZE;
const NUM_METADATA_COLUMNS: usize = 4;

const METADATA_INDIRECTION: usize = 0;
const METADATA_RID: usize = 1;
const METADATA_TIMESTAMP: usize = 2;
const METADATA_SCHEMA_ENCODING: usize = 3;

#[derive(Clone, Copy, Debug, Default)]
struct RID {
    rid: u64,
}

impl RID {
    fn new(rid: u64) -> Self {
        RID { rid }
    }

    fn next(&self) -> RID {
        RID { rid: self.rid + 1 }
    }

    fn slot(&self) -> usize {
        (self.rid & 0b111111111) as usize
    }

    fn page(&self) -> usize {
        ((self.rid >> 9) & 0b1111) as usize
    }

    fn page_range(&self) -> usize {
        (self.rid >> 13) as usize
    }

    fn raw(&self) -> u64 {
        self.rid
    }
}

impl From<u64> for RID {
    fn from(value: u64) -> Self {
        RID { rid: value }
    }
}

#[derive(Clone, Debug, Default)]
#[pyclass(subclass)]
//change to BTreeMap when we need to implement ranges
struct Index {
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
    pub fn range_from_index(
        &self,
        column_number: usize,
        begin: i64,
        end: i64,
    ) -> Option<Vec<&RID>> {
        self.indices[column_number].as_ref().map(|map| {
            map.range(begin..end)
                .flat_map(|item| item.1)
                .collect::<Vec<&RID>>()
        })
    }
    pub fn create_index(&mut self, column_number: usize) {
        self.indices[column_number] = Some(BTreeMap::new());
    }

    pub fn drop_index(&mut self, column_number: usize) {
        self.indices[column_number] = None;
    }
}

#[derive(Clone, Debug)]
#[pyclass(subclass, get_all)]
struct Record {
    rid: u64,
    indirection: u64,
    schema_encoding: u64,
    columns: Py<PyList>,
}

#[pymethods]
impl Record {
    #[new]
    pub fn new(rid: u64, indirection: u64, schema_encoding: u64, columns: Py<PyList>) -> Self {
        Record {
            rid: 0,
            indirection: 0,
            schema_encoding: 0,
            columns,
        }
    }
}

#[derive(Debug)]
struct PhysicalPage {
    page: RefCell<[i64; PAGE_SLOTS]>,
}

impl Default for PhysicalPage {
    fn default() -> Self {
        PhysicalPage {
            page: RefCell::new([0; PAGE_SLOTS]),
        }
    }
}

impl PhysicalPage {
    fn slot(&self, index: usize) -> i64 {
        self.page.borrow()[index]
    }

    fn write_slot(&self, index: usize, value: i64) {
        self.page.borrow_mut()[index] = value;
    }
}

#[derive(Debug)]
struct Page {
    columns: Box<[PhysicalPage]>,
}

impl Page {
    pub fn new(num_columns: usize) -> Self {
        let mut columns: Vec<PhysicalPage> = Vec::with_capacity(NUM_METADATA_COLUMNS + num_columns);
        columns.resize_with(NUM_METADATA_COLUMNS + num_columns, Default::default);
        let columns = columns.into_boxed_slice();

        Page { columns }
    }

    pub fn get_column(&self, index: usize) -> &PhysicalPage {
        self.columns.as_ref()[index].borrow()
    }
}

#[derive(Debug)]
struct PageRange {
    base_pages: Vec<Page>,
    tail_pages: Vec<Page>,
}

impl PageRange {
    pub fn new(num_columns: usize) -> Self {
        let tail_pages: Vec<Page> = vec![Page::new(num_columns)];
        let mut base_pages: Vec<Page> = Vec::new();

        for _ in 0..RANGE_PAGE_COUNT {
            base_pages.push(Page::new(num_columns));
        }

        PageRange {
            base_pages,
            tail_pages,
        }
    }

    pub fn get_page(&self, page_num: usize) -> &Page {
        &self.base_pages[page_num]
    }
}

#[derive(Debug, Default)]
#[pyclass(subclass)]
struct Table {
    name: String,
    num_columns: usize,
    primary_key_index: usize,
    index: Index,
    next_rid: RID,
    ranges: Vec<PageRange>,
}

impl Table {
    fn get_page_range(&self, range_number: usize) -> &PageRange {
        &self.ranges[range_number]
    }
    fn find_rows(&self, column_index: usize, value: i64) -> Vec<RID> {
        match self.index.get_from_index(column_index, value) {
            Some(vals) => vals,
            None => {
                let rid: RID = 0.into();
                let mut rids = Vec::new();
                while rid.raw() < self.next_rid.raw() {
                    let page = self.get_page_range(rid.page_range()).get_page(rid.page());

                    if page
                        .get_column(NUM_METADATA_COLUMNS + column_index)
                        .slot(rid.slot())
                        == value
                    {
                        rids.push(rid.clone());
                    }
                }
                rids
            }
        }
    }
}

#[pymethods]
impl Table {
    #[new]
    pub fn new(name: String, num_columns: usize, key_index: usize) -> Table {
        Table {
            name,
            num_columns,
            primary_key_index: key_index,
            index: Index::new(key_index, num_columns),
            next_rid: 0.into(),
            ranges: Vec::new(),
        }
    }

    pub fn sum(&self, start_range: i64, end_range: i64, column_index: usize) -> i64 {
        let mut rid: RID = 0.into();
        let mut sum: i64 = 0;

        while rid.raw() < self.next_rid.raw() {
            let page = self.get_page_range(rid.page_range()).get_page(rid.page());

            let key = page
                .get_column(NUM_METADATA_COLUMNS + self.primary_key_index)
                .slot(rid.slot());

            if key >= start_range && key < end_range {
                sum += page
                    .get_column(NUM_METADATA_COLUMNS + column_index)
                    .slot(rid.slot());
            }

            rid = rid.next();
        }

        sum
    }

    pub fn select(&self, search_value: i64, column_index: usize, columns: &PyList) -> Py<PyList> {
        let selected_records = Python::with_gil(|py| -> Py<PyList> { PyList::empty(py).into() });

        let included_columns: Vec<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_i, x)| x.extract::<i64>().unwrap() != 0)
            .map(|(i, _x)| i)
            .collect();

        let vals: Vec<RID> = self.find_rows(column_index, search_value);

        Python::with_gil(|py| {
            let result_cols = PyList::empty(py);
            for rid in vals {
                let page = self.get_page_range(rid.page_range()).get_page(rid.page());
                for i in included_columns.iter() {
                    result_cols.append(page.get_column(NUM_METADATA_COLUMNS + i).slot(rid.slot()));
                }
                let record = PyCell::new(
                    py,
                    Record::new(
                        page.get_column(METADATA_RID).slot(rid.slot()) as u64,
                        page.get_column(METADATA_INDIRECTION).slot(rid.slot()) as u64,
                        page.get_column(METADATA_SCHEMA_ENCODING).slot(rid.slot()) as u64,
                        result_cols.into(),
                    ),
                )
                .unwrap();

                selected_records.as_ref(py).append(record);
            }
        });

        selected_records
    }

    #[args(values = "*")]
    pub fn insert(&mut self, values: &PyTuple) {
        let rid: RID = self.next_rid;
        let page_range = rid.page_range();
        let page = rid.page();
        let slot = rid.slot();

        if self.ranges.len() <= page_range {
            self.ranges.push(PageRange::new(self.num_columns));
        }

        let page_range = self.get_page_range(page_range);
        let page = page_range.get_page(page);

        page.get_column(METADATA_RID)
            .write_slot(slot, rid.raw() as i64);

        for (i, val) in values.iter().enumerate() {
            page.get_column(NUM_METADATA_COLUMNS + i)
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

#[derive(Clone, Debug, Default)]
#[pyclass(subclass)]
struct CrabStore {
    tables: HashMap<String, Py<Table>>,
}

#[pymethods]
impl CrabStore {
    #[new]
    pub fn new() -> Self {
        CrabStore {
            tables: HashMap::new(),
        }
    }

    pub fn create_table(
        &mut self,
        name: String,
        num_columns: usize,
        key_index: usize,
    ) -> Py<Table> {
        Python::with_gil(|py| -> Py<Table> {
            let table: Py<Table> =
                Py::new(py, Table::new(name.clone(), num_columns, key_index)).unwrap();
            self.tables.insert(name.clone(), Py::clone_ref(&table, py));
            table
        })
    }

    pub fn drop_table(&mut self, name: String) -> PyResult<()> {
        self.tables.remove(&name);
        Ok(())
    }

    pub fn get_table(&self, name: String) -> Py<Table> {
        Py::clone(self.tables.get(&name).unwrap())
    }
}

/// A Python module implemented in Rust.
#[pymodule]
fn crabstore(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<Record>()?;
    m.add_class::<Table>()?;
    m.add_class::<CrabStore>()?;
    // m.add_function(wrap_pyfunction!(sum_as_string, m)?)?;
    Ok(())
}
