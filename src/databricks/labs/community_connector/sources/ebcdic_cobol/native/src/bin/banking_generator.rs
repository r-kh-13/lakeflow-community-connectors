use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TRANSACTION_RECORD_SIZE: usize = 64;
const TRANSACTIONS_PER_FILE: u64 = 50_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        return Err("usage: banking_generator OUTPUT_DIRECTORY TRANSACTION_COUNT".into());
    }
    let root = PathBuf::from(&args[1]);
    let transaction_count: u64 = args[2].parse()?;
    fs::create_dir_all(root.join("data/transactions_f"))?;
    fs::create_dir_all(root.join("data/payments_v"))?;
    fs::create_dir_all(root.join("data/ledger_vb"))?;
    fs::create_dir_all(root.join("data/transactions_gzip"))?;
    fs::create_dir_all(root.join("data/edge_values"))?;
    fs::create_dir_all(root.join("data/corrupt_rdw"))?;
    fs::create_dir_all(root.join("copybooks"))?;

    write_copybooks(&root)?;
    let transaction_sum_cents = write_transactions(&root, transaction_count)?;
    write_gzip_copy(&root)?;
    let payment_sum_cents = write_payments(&root, 10_000)?;
    let (debit_sum_cents, credit_sum_cents) = write_ledger(&root, 10_000)?;
    write_edge_values(&root)?;
    File::create(root.join("data/corrupt_rdw/zero-rdw.dat"))?.write_all(&[0, 0, 0, 0])?;

    let oracle = format!(
        concat!(
            "{{\n",
            "  \"transaction_count\": {transaction_count},\n",
            "  \"transaction_sum_cents\": {transaction_sum_cents},\n",
            "  \"transaction_files\": {transaction_files},\n",
            "  \"payment_count\": 10000,\n",
            "  \"payment_sum_cents\": {payment_sum_cents},\n",
            "  \"ledger_count\": 10000,\n",
            "  \"ledger_debit_sum_cents\": {debit_sum_cents},\n",
            "  \"ledger_credit_sum_cents\": {credit_sum_cents},\n",
            "  \"edge_count\": 6,\n",
            "  \"compressed_transaction_count\": 50000,\n",
            "  \"edge_null_amounts\": 1,\n",
            "  \"transaction_record_size\": {TRANSACTION_RECORD_SIZE}\n",
            "}}\n"
        ),
        transaction_count = transaction_count,
        transaction_sum_cents = transaction_sum_cents,
        transaction_files = transaction_count.div_ceil(TRANSACTIONS_PER_FILE),
        payment_sum_cents = payment_sum_cents,
        debit_sum_cents = debit_sum_cents,
        credit_sum_cents = credit_sum_cents,
        TRANSACTION_RECORD_SIZE = TRANSACTION_RECORD_SIZE,
    );
    fs::write(root.join("oracle.json"), oracle)?;
    println!(
        "generated transactions={transaction_count} transaction_sum_cents={transaction_sum_cents}"
    );
    Ok(())
}

fn write_gzip_copy(root: &Path) -> std::io::Result<()> {
    let input = File::open(root.join("data/transactions_f/part-00000.dat"))?;
    let output = File::create(root.join("data/transactions_gzip/part-00000.dat.gz"))?;
    let status = Command::new("gzip")
        .arg("-c")
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output))
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other("gzip command failed"));
    }
    Ok(())
}

fn write_transactions(root: &Path, count: u64) -> std::io::Result<i128> {
    let mut sum = 0i128;
    let mut file_index = u64::MAX;
    let mut writer: Option<BufWriter<File>> = None;
    for index in 0..count {
        let next_file = index / TRANSACTIONS_PER_FILE;
        if next_file != file_index {
            file_index = next_file;
            writer = Some(BufWriter::new(File::create(
                root.join(format!("data/transactions_f/part-{file_index:05}.dat")),
            )?));
        }
        let amount_cents = transaction_amount(index);
        sum += i128::from(amount_cents);
        let mut record = Vec::with_capacity(TRANSACTION_RECORD_SIZE);
        write_comp(&mut record, index + 1, 8);
        write_display(&mut record, (index * 7919) % 10_000_000_000, 10);
        write_text(
            &mut record,
            if index.is_multiple_of(50) {
                "RV"
            } else if index.is_multiple_of(20) {
                "RF"
            } else if index.is_multiple_of(10) {
                "TR"
            } else {
                "PY"
            },
            2,
        );
        write_text(
            &mut record,
            if index % 10 < 7 {
                "CAD"
            } else if index % 10 < 9 {
                "USD"
            } else {
                "EUR"
            },
            3,
        );
        write_comp3(&mut record, amount_cents, 13);
        let fee_cents = if amount_cents < 0 {
            0
        } else {
            (amount_cents / 400).clamp(25, 99_999)
        };
        write_comp3(&mut record, fee_cents, 7);
        let balance_cents = 10_000_000_000i64 + ((index as i64 * 104_729) % 90_000_000_000);
        write_comp3(&mut record, balance_cents, 15);
        write_text(
            &mut record,
            match index % 100 {
                0..=54 => "MOB",
                55..=79 => "WEB",
                80..=94 => "POS",
                _ => "ATM",
            },
            4,
        );
        write_text(
            &mut record,
            match index % 20 {
                0..=13 => "CA",
                14..=16 => "US",
                17 => "GB",
                18 => "FR",
                _ => "DE",
            },
            2,
        );
        write_comp(&mut record, (index * 37) % 1000, 2);
        write_text(&mut record, &format!("{:06}", (index * 97) % 1_000_000), 6);
        write_display(&mut record, 20260903, 8);
        assert_eq!(record.len(), TRANSACTION_RECORD_SIZE);
        writer
            .as_mut()
            .expect("writer initialized")
            .write_all(&record)?;
    }
    if let Some(mut writer) = writer {
        writer.flush()?;
    }
    Ok(sum)
}

fn transaction_amount(index: u64) -> i64 {
    if index.is_multiple_of(100_000) {
        9_000_000_000_000
    } else {
        let base = ((index * 48_271) % 2_500_000) as i64 + 100;
        if index.is_multiple_of(50) {
            -base
        } else {
            base
        }
    }
}

fn write_payments(root: &Path, count: u64) -> std::io::Result<i128> {
    let path = root.join("data/payments_v/payments-rdw.dat");
    let mut writer = BufWriter::new(File::create(path)?);
    let mut sum = 0i128;
    for index in 0..count {
        let leg_count = (index % 3 + 1) as usize;
        let mut payload = Vec::new();
        write_comp(&mut payload, index + 1, 8);
        write_display(&mut payload, leg_count as u64, 1);
        for leg in 0..leg_count {
            write_text(
                &mut payload,
                match (index + leg as u64) % 4 {
                    0 => "ROY",
                    1 => "TD",
                    2 => "BMO",
                    _ => "NBC",
                },
                4,
            );
            let amount = (((index + 1) * (leg as u64 + 3) * 137) % 50_000_000) as i64;
            sum += i128::from(amount);
            write_comp3(&mut payload, amount, 11);
        }
        write_text(
            &mut payload,
            if index.is_multiple_of(97) { "RJ" } else { "OK" },
            2,
        );
        writer.write_all(&(payload.len() as u16).to_be_bytes())?;
        writer.write_all(&[0, 0])?;
        writer.write_all(&payload)?;
    }
    writer.flush()?;
    Ok(sum)
}

fn write_ledger(root: &Path, count: u64) -> std::io::Result<(i128, i128)> {
    let path = root.join("data/ledger_vb/ledger-vb.dat");
    let mut writer = BufWriter::new(File::create(path)?);
    let mut debit_sum = 0i128;
    let mut credit_sum = 0i128;
    for block_start in (0..count).step_by(100) {
        let block_count = usize::try_from((count - block_start).min(100)).unwrap();
        let mut block = Vec::with_capacity(4 + block_count * 58);
        block.extend_from_slice(&[0, 0, 0, 0]);
        for offset in 0..block_count {
            let index = block_start + offset as u64;
            let amount = ((index * 65_537) % 90_000_000) as i64;
            let (debit, credit) = if index.is_multiple_of(2) {
                (amount, 0)
            } else {
                (0, amount)
            };
            debit_sum += i128::from(debit);
            credit_sum += i128::from(credit);
            let mut payload = Vec::with_capacity(50);
            write_comp(&mut payload, index + 1, 8);
            write_text(&mut payload, &format!("GL{:06}", index % 1_000_000), 8);
            write_comp3(&mut payload, debit, 13);
            write_comp3(&mut payload, credit, 13);
            write_text(
                &mut payload,
                if index.is_multiple_of(997) {
                    "MANUAL ADJUSTMENT"
                } else {
                    "DAILY SETTLEMENT"
                },
                20,
            );
            assert_eq!(payload.len(), 50);
            block.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            block.extend_from_slice(&[0, 0]);
            block.extend_from_slice(&payload);
        }
        let block_len = block.len() as u16;
        block[0..2].copy_from_slice(&block_len.to_be_bytes());
        writer.write_all(&block)?;
    }
    writer.flush()?;
    Ok((debit_sum, credit_sum))
}

fn write_edge_values(root: &Path) -> std::io::Result<()> {
    let path = root.join("data/edge_values/edge-values.dat");
    let mut writer = BufWriter::new(File::create(path)?);
    let values = [0, 1, -1, 999_999_999, -999_999_999];
    for (index, value) in values.into_iter().enumerate() {
        write_display_to(&mut writer, (index + 1) as u64, 3)?;
        let mut packed = Vec::new();
        write_comp3(&mut packed, value, 9);
        writer.write_all(&packed)?;
    }
    write_display_to(&mut writer, 6, 3)?;
    writer.write_all(&[0x12, 0xA4, 0x56, 0x78, 0x9C])?;
    Ok(())
}

fn write_copybooks(root: &Path) -> std::io::Result<()> {
    fs::write(
        root.join("copybooks/transactions.cpy"),
        concat!(
            "01 TRANSACTION-RECORD.\n",
            " 05 TRANSACTION-ID PIC 9(12) COMP.\n",
            " 05 ACCOUNT-ID PIC 9(10).\n",
            " 05 EVENT-TYPE PIC X(2).\n",
            " 05 CURRENCY PIC X(3).\n",
            " 05 AMOUNT PIC S9(11)V9(2) COMP-3.\n",
            " 05 FEE PIC S9(5)V9(2) COMP-3.\n",
            " 05 BALANCE PIC S9(13)V9(2) COMP-3.\n",
            " 05 CHANNEL PIC X(4).\n",
            " 05 COUNTRY PIC X(2).\n",
            " 05 RISK-SCORE PIC 9(3) COMP.\n",
            " 05 AUTH-CODE PIC X(6).\n",
            " 05 POSTING-DATE PIC 9(8).\n",
        ),
    )?;
    fs::write(
        root.join("copybooks/payments.cpy"),
        concat!(
            "01 PAYMENT-RECORD.\n",
            " 05 PAYMENT-ID PIC 9(10) COMP.\n",
            " 05 LEG-COUNT PIC 9(1).\n",
            " 05 LEGS OCCURS 1 TO 3 TIMES DEPENDING ON LEG-COUNT.\n",
            "  10 BANK-CODE PIC X(4).\n",
            "  10 LEG-AMOUNT PIC S9(9)V9(2) COMP-3.\n",
            " 05 STATUS PIC X(2).\n",
        ),
    )?;
    fs::write(
        root.join("copybooks/ledger.cpy"),
        concat!(
            "01 LEDGER-RECORD.\n",
            " 05 ENTRY-ID PIC 9(10) COMP.\n",
            " 05 GL-CODE PIC X(8).\n",
            " 05 DEBIT PIC S9(11)V9(2) COMP-3.\n",
            " 05 CREDIT PIC S9(11)V9(2) COMP-3.\n",
            " 05 DESCRIPTION PIC X(20).\n",
        ),
    )?;
    fs::write(
        root.join("copybooks/edge-values.cpy"),
        concat!(
            "01 EDGE-RECORD.\n",
            " 05 EDGE-ID PIC 9(3).\n",
            " 05 EDGE-AMOUNT PIC S9(7)V9(2) COMP-3.\n",
        ),
    )?;
    Ok(())
}

fn write_text(output: &mut Vec<u8>, value: &str, width: usize) {
    let mut count = 0usize;
    for character in value.chars().take(width) {
        output.push(encode_cp037(character));
        count += 1;
    }
    output.extend(std::iter::repeat_n(0x40, width - count));
}

fn write_display(output: &mut Vec<u8>, value: u64, width: usize) {
    for character in format!("{value:0width$}").chars() {
        output.push(encode_cp037(character));
    }
}

fn write_display_to(writer: &mut impl Write, value: u64, width: usize) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(width);
    write_display(&mut bytes, value, width);
    writer.write_all(&bytes)
}

fn write_comp(output: &mut Vec<u8>, value: u64, width: usize) {
    output.extend_from_slice(&value.to_be_bytes()[8 - width..]);
}

fn write_comp3(output: &mut Vec<u8>, value: i64, precision: usize) {
    let mut nibbles = format!("{:0precision$}", value.unsigned_abs())
        .bytes()
        .map(|value| value - b'0')
        .collect::<Vec<_>>();
    nibbles.push(if value < 0 { 0x0D } else { 0x0C });
    if !nibbles.len().is_multiple_of(2) {
        nibbles.insert(0, 0);
    }
    for pair in nibbles.as_chunks::<2>().0 {
        output.push((pair[0] << 4) | pair[1]);
    }
}

fn encode_cp037(value: char) -> u8 {
    match value {
        ' ' => 0x40,
        '-' => 0x60,
        '0'..='9' => 0xF0 + (value as u8 - b'0'),
        'A'..='I' => 0xC1 + (value as u8 - b'A'),
        'J'..='R' => 0xD1 + (value as u8 - b'J'),
        'S'..='Z' => 0xE2 + (value as u8 - b'S'),
        _ => panic!("unsupported generator character: {value}"),
    }
}
