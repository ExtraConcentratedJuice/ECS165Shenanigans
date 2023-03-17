#![feature(test)]
extern crate test;
use core::num;
use crabcore::{
    crabstore::CrabStore,
    transaction::{Query, Transaction},
    transaction_worker::TransactionWorker,
};
use rand::prelude::*;
use std::{collections::HashMap, path::Path};
use tempfile::tempdir;
use test::Bencher;

#[test]
fn transaction_test() {
    let dir = tempdir().unwrap();
    transaction_test1(dir.path());
    transaction_test2(dir.path());
}

fn transaction_test2(dir: &Path) {
    let mut rand = StdRng::seed_from_u64(3562901);
    let mut crabstore = CrabStore::new(dir.into());
    crabstore.open();

    let grades = crabstore.get_table("Grades");
    let mut records: HashMap<u64, Vec<u64>> = HashMap::new();

    let number_of_records = 1000;
    let number_of_transactions = 2;
    let number_of_operations_per_record = 10;
    let num_threads = 8;

    let mut keys: Vec<u64> = Vec::new();
    let mut transactions = Vec::new();

    for _ in 0..number_of_transactions {
        transactions.push(Transaction::new());
    }

    for i in 0..number_of_records {
        let key = 92106429 + i;
        keys.push(key);
        let cols = vec![
            key,
            rand.gen_range((i * 20)..((i + 1) * 20)),
            rand.gen_range((i * 20)..((i + 1) * 20)),
            rand.gen_range((i * 20)..((i + 1) * 20)),
            rand.gen_range((i * 20)..((i + 1) * 20)),
        ];
        records.insert(key, cols.clone());
    }

    for key in keys.iter() {
        let record = &grades.select_query(*key, 0, &[1, 1, 1, 1, 1], None)[0].columns;

        for (i, col) in record.iter().enumerate() {
            assert_eq!(*col, records.get(key).unwrap()[i]);
        }
    }

    let mut workers: Vec<TransactionWorker> = Vec::new();

    for _ in 0..num_threads {
        workers.push(TransactionWorker::new());
    }

    for i in (0..number_of_operations_per_record).rev() {
        for key in keys.iter() {
            let mut updated_cols = [None, None, None, None, None];
            for i in 2..grades.columns() {
                let value = rand.gen_range(0..20);
                updated_cols[i] = Some(value);

                records.get_mut(key).unwrap()[i] = value;
                transactions[(*key % number_of_transactions) as usize]
                    .add_query(Query::Select(*key, 0, Box::new([1, 1, 1, 1, 1])), &grades);

                transactions[(*key % number_of_transactions) as usize]
                    .add_query(Query::Update(*key, Box::new(updated_cols)), &grades);
            }
        }
    }

    for i in 0..number_of_transactions {
        workers
            .get_mut((i % num_threads) as usize)
            .unwrap()
            .add_transaction(transactions.remove(0));
    }

    for worker in workers.iter_mut() {
        worker.run();
    }

    for worker in workers.iter_mut() {
        worker.join();
    }

    let mut score = keys.len();

    for key in keys.iter() {
        let record = &grades.select_query(*key, 0, &[1, 1, 1, 1, 1], None)[0].columns;

        for (i, col) in record.iter().enumerate() {
            if *col != records.get(key).unwrap()[i] {
                score -= 1;
                println!(
                    "Select Error: Key {} | Result: {:?} | Correct: {:?}",
                    *key,
                    record,
                    records.get(key).unwrap()
                );
                break;
            }
        }
    }

    println!("Score: {score}/{}", keys.len());

    crabstore.close();
}

fn transaction_test1(dir: &Path) {
    let mut rand = StdRng::seed_from_u64(3562901);

    let mut crabstore = CrabStore::new(dir.into());

    let grades = crabstore.create_table("Grades", 5, 0);

    let mut records: HashMap<u64, Vec<u64>> = HashMap::new();

    let number_of_records = 1000;
    let number_of_transactions = 100;
    let num_threads = 8;

    grades.build_index(2);
    grades.build_index(3);
    grades.build_index(4);

    let mut keys: Vec<u64> = Vec::new();
    let mut insert_transactions = Vec::new();

    for _ in 0..number_of_transactions {
        insert_transactions.push(Transaction::new());
    }

    for i in 0..number_of_records {
        let key = 92106429 + i;
        keys.push(key);
        let cols = vec![
            key,
            rand.gen_range((i * 20)..((i + 1) * 20)),
            rand.gen_range((i * 20)..((i + 1) * 20)),
            rand.gen_range((i * 20)..((i + 1) * 20)),
            rand.gen_range((i * 20)..((i + 1) * 20)),
        ];
        records.insert(key, cols.clone());

        insert_transactions[(i % number_of_transactions) as usize]
            .add_query(Query::Insert(cols.into()), &grades);
    }

    let mut workers: Vec<TransactionWorker> = Vec::new();

    for _ in 0..num_threads {
        workers.push(TransactionWorker::new());
    }

    for i in (0..number_of_transactions).rev() {
        workers[(i % num_threads) as usize].add_transaction(insert_transactions.remove(i as usize));
    }

    for worker in workers.iter_mut() {
        worker.run();
    }

    for worker in workers.iter_mut() {
        worker.join();
    }

    for key in keys {
        let record = &grades.select_query(key, 0, &[1, 1, 1, 1, 1], None)[0].columns;

        for (i, col) in record.iter().enumerate() {
            assert_eq!(*col, records.get(&key).unwrap()[i]);
        }
    }

    crabstore.close();
}
