use std::sync::Arc;

use crate::{
    table::{Table, TableData},
    transaction::Transaction,
};
#[derive(Debug)]
pub struct TransactionWorker {
    transactions: Vec<Transaction>,
    table: Arc<TableData>,
}
