//! The cost of the pool's per-sender views: each sender mempool starts with a copy of the
//! state's wallets. Measured, not asserted (ignored; `make pool-bench` runs it in a release
//! build):
//!
//! - the time of `WalletRepository::clone` at 10^3 to 10^6 wallets (median of 20) and the
//!   memory four copies add;
//! - the time of a re-add with 1 to 15,000 senders at 10^4 and 10^5 wallets (at least one wallet
//!   per sender), and the memory its views hold.
//!
//! Each case runs in a child process of its own, so that no case reads memory another one freed.
//! A case whose views could hold more than the memory budget (4 GiB; `HEARTWOOD_POOL_BENCH_MIB`
//! changes it), at twice the extrapolation from the memory per view of the largest case run at
//! the same wallet count, is not run; its time and memory are printed as extrapolations.
//!
//! The thresholds for a follow-up are a copy at 10^5 wallets above 5 ms, or a re-add of 1,000
//! senders at 10^4 wallets above 100 ms.

mod support;

use std::hint::black_box;
use std::process::Command;

use heartwood_crypto::Transaction;
use heartwood_crypto::identities::Address;
use heartwood_pool::{DelegateVersions, Pool, PoolContext, Storage, StoredTransaction};
use heartwood_state::wallets::WalletRepository;
use support::*;

/// The variable that selects the case a child process runs.
const CASE: &str = "HEARTWOOD_POOL_BENCH_CASE";

/// Resident memory of this process, in MiB.
fn resident_mib() -> f64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|field| field.parse().ok())
        .unwrap_or(0);
    (pages * 4096) as f64 / 1_048_576.0
}

/// Milliseconds since an arbitrary start.
#[expect(
    clippy::disallowed_types,
    reason = "a benchmark measures wall time; the pool itself never reads it"
)]
fn now_ms() -> f64 {
    use std::sync::LazyLock;
    use std::time::Instant;
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_secs_f64() * 1_000.0
}

/// `count` wallets with balances; every tenth has sent a transaction (a bound key), as on a
/// chain where most wallets only receive. The first `senders` are the returned accounts'
/// wallets, funded and keyed.
fn wallets(count: usize, senders: usize) -> (WalletRepository, Vec<Account>) {
    let mut wallets = WalletRepository::new(NETWORK);
    let mut accounts = Vec::with_capacity(senders);
    for n in 0..count {
        let keyed = n < senders || n % 10 == 0;
        let address = if keyed {
            let account = Account::new(&format!("bench wallet {n}"));
            let address = account.address;
            wallets.bind_public_key(&address, account.public_key().clone());
            if n < senders {
                accounts.push(account);
            }
            address
        } else {
            let mut bytes = [0u8; 21];
            bytes[0] = NETWORK;
            bytes[1..9].copy_from_slice(&(n as u64).to_le_bytes());
            Address::from_bytes(bytes)
        };
        let wallet = wallets.get_or_create(&address);
        wallet.credit(1_000 * FEE).unwrap();
        wallet.set_nonce(u64::from(keyed));
    }
    (wallets, accounts)
}

/// Run `case` in a child process and return the line it prints after `RESULT`.
fn child(case: &str) -> String {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "view_cost_case", "--ignored", "--nocapture"])
        .env(CASE, case)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("RESULT "))
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("case {case} gave no result: {stdout}"))
}

/// The measurements, each case in its own process.
#[test]
#[ignore = "a measurement: make pool-bench"]
fn view_cost() {
    let budget_mib: f64 = std::env::var("HEARTWOOD_POOL_BENCH_MIB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4_096.0);
    println!("wallets     copy (median of 20)   memory per copy");
    for count in [1_000, 10_000, 100_000, 1_000_000] {
        println!("{}", child(&format!("copy:{count}")));
    }
    println!("wallets   senders   re-add          memory of the views");
    for base in [10_000, 100_000] {
        let mut per_view: Option<(f64, f64)> = None;
        for senders in [1, 100, 1_000, 15_000] {
            let count = base.max(senders);
            // Twice the extrapolation must fit, and a case without one runs only when small.
            let fits = match per_view {
                Some((_, mib)) => 2.0 * mib * senders as f64 <= budget_mib,
                None => senders * count <= 10_000_000,
            };
            if !fits {
                let (ms, mib) = per_view.unwrap_or((f64::NAN, f64::NAN));
                let (ms, mib) = (ms * senders as f64, mib * senders as f64);
                println!(
                    "{count:>7} {senders:>9}   ~{ms:>9.0} ms   ~{mib:>9.0} MiB (not run: over \
                     the {budget_mib:.0} MiB budget; extrapolated)"
                );
                continue;
            }
            let line = child(&format!("readd:{count}:{senders}"));
            let fields: Vec<f64> = line
                .split_whitespace()
                .filter_map(|field| field.parse().ok())
                .collect();
            if let [.., ms, mib] = fields[..]
                && senders >= 100
            {
                per_view = Some((ms / senders as f64, mib / senders as f64));
            }
            println!("{line}");
        }
    }
}

/// One case, in a child process of [`view_cost`].
#[test]
#[ignore = "run by view_cost in a child process"]
fn view_cost_case() {
    let Ok(case) = std::env::var(CASE) else {
        return;
    };
    let numbers: Vec<usize> = case
        .split(':')
        .skip(1)
        .filter_map(|n| n.parse().ok())
        .collect();
    match (case.split(':').next(), numbers.as_slice()) {
        (Some("copy"), &[count]) => copy_case(count),
        (Some("readd"), &[count, senders]) => readd_case(count, senders),
        _ => panic!("unknown case {case}"),
    }
}

fn copy_case(count: usize) {
    let (wallets, _) = wallets(count, 0);
    let mut samples = Vec::new();
    for _ in 0..20 {
        let start = now_ms();
        let copy = black_box(wallets.clone());
        samples.push(now_ms() - start);
        drop(copy);
    }
    samples.sort_by(f64::total_cmp);
    let before = resident_mib();
    let copies: Vec<WalletRepository> = (0..4).map(|_| wallets.clone()).collect();
    let mib = (resident_mib() - before) / 4.0;
    black_box(copies);
    println!(
        "RESULT {count:>9}   {:>12.3} ms   {mib:>12.2} MiB",
        samples[10]
    );
}

fn readd_case(count: usize, senders: usize) {
    let (wallets, accounts) = wallets(count, senders);
    // Each transfer goes to a wallet the state does not have.
    let recipient = Account::new("bench recipient");
    let dir = tempdir("bench");
    let options = options(&dir);
    {
        let mut storage = Storage::open(&options.storage, None).unwrap();
        for sender in &accounts {
            let tx: Transaction = sender.transfer(&recipient, 1, 2, FEE);
            storage
                .add(&StoredTransaction {
                    height: TIP,
                    id: *tx.id(),
                    recipient_id: recipient.address.to_base58(),
                    sender_id: sender.address.to_base58(),
                    serialised: tx.bytes().to_vec(),
                })
                .unwrap();
        }
    }
    let mut pool = Pool::open(options, None).unwrap();
    let online = DelegateVersions::new();
    let ctx = PoolContext {
        wallets: &wallets,
        tip: TIP,
        milestones: &MILESTONES,
        network: &NETWORK_FILE,
        online: &online,
    };
    let before = resident_mib();
    let start = now_ms();
    let readded = pool.readd(&ctx, &[], true).unwrap();
    let ms = now_ms() - start;
    let mib = resident_mib() - before;
    assert_eq!(
        readded.stored.len(),
        senders,
        "{:?}",
        readded.failed.first()
    );
    println!("RESULT {count:>7} {senders:>9}   {ms:>10.1} ms   {mib:>10.1} MiB");
    drop(pool);
    let _ = std::fs::remove_dir_all(dir);
}
