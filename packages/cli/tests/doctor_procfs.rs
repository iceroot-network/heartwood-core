//! `/proc` and `/sys`: the parsers against tables captured on a Linux host (cleaned: documentation
//! addresses, a neutral user), hand-written rows for what that host lacks, the real `HostProc`
//! on this host, and the view's reading of a partition's parent device.

mod doctor_support;

use std::ffi::OsString;
use std::fs;
use std::net::{Ipv6Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};

use doctor_support::{FakeWorld, scope};
use heartwood_cli::doctor::seams::{FsError, ProcFile, ProcView};
use heartwood_cli::doctor::sources::ParseError;
use heartwood_cli::doctor::sources::procfs::{
    BlockDevice, HostProc, MountRow, TcpState, doctor_mcp_servers, is_doctor_mcp, parse_meminfo,
    parse_mountinfo, parse_net_tcp, parse_os_release, parse_rotational, parse_stat_start,
    parse_status, socket_inodes,
};
use heartwood_cli::doctor::view::{Gathered, Reason, View};
use heartwood_cli::doctor::{DoctorOptions, HostWorld};

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doctor");
    fs::read(path.join(name)).unwrap()
}

fn socket(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

#[test]
fn tcp_rows_give_addresses_ports_states_owners_and_inodes() {
    let rows = parse_net_tcp(&fixture("proc/net_tcp"), false).unwrap();
    assert_eq!(rows.len(), 13);
    let first = rows[0];
    assert_eq!(first.local, socket("0.0.0.0:8123"));
    assert_eq!(first.remote, socket("0.0.0.0:0"));
    assert_eq!(
        (first.state, first.uid, first.inode),
        (TcpState::Listen, 0, 7644)
    );
    assert_eq!(rows[1].local, socket("127.0.0.1:3291"));
    assert_eq!(rows[1].uid, 1000);
    let established = rows[5];
    assert_eq!(established.state, TcpState::Established);
    assert_eq!(established.local, socket("127.0.0.11:6002"));
    assert_eq!(established.remote, socket("127.0.0.12:41803"));
    assert_eq!(established.inode, 48_701_887);
    assert_eq!(rows[7].state, TcpState::Other(8));
    assert_eq!(rows[7].local, socket("192.0.2.10:47856"));
    assert_eq!(rows[11].state, TcpState::Other(6));
    let listening = rows.iter().filter(|row| row.state == TcpState::Listen);
    assert_eq!(listening.count(), 5);
}

#[test]
fn tcp6_rows_keep_ipv6_and_mapped_addresses() {
    let rows = parse_net_tcp(&fixture("proc/net_tcp6"), true).unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0].local, socket("[::]:8123"));
    assert_eq!(rows[4].local, socket("[::1]:8080"));
    assert_eq!(rows[4].uid, 1000);
    let mapped = "   0: 0000000000000000FFFF00000100007F:1772 0000000000000000FFFF00000C00007F:9C40 01 \
                  00000000:00000000 00:00000000 00000000  1000        0 555 1 0 20 4 30 10 -1\n";
    let rows = parse_net_tcp(mapped.as_bytes(), true).unwrap();
    let local = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 1);
    assert_eq!(rows[0].local, SocketAddr::new(local.into(), 6002));
    assert!(rows[0].local.is_ipv6());
    assert_eq!(rows[0].remote.port(), 40000);
}

#[test]
fn tcp_tables_that_are_not_the_kernels_are_refused() {
    let bad = [
        ("   0: 0100007F:1772\n", false),
        (
            "   0: 0100007F:1772 00000000:0000 0A 0 0 0 1000 0 x\n",
            false,
        ),
        (
            "   0: 0100007F:17720 00000000:0000 0A 0 0 0 1000 0 7\n",
            false,
        ),
        (
            "   0: 0100007F:1772 00000000:0000 0A 0 0 0 1000 0 7\n",
            true,
        ),
        (
            "   0: 0100007Fé:1772 00000000:0000 0A 0 0 0 1000 0 7\n",
            false,
        ),
        (
            "   0: 0100007F:1772 00000000:0000 ZZ 0 0 0 1000 0 7\n",
            false,
        ),
    ];
    for (text, v6) in bad {
        let parsed = parse_net_tcp(text.as_bytes(), v6);
        assert_eq!(parsed, Err(ParseError::Malformed("tcp row")), "{text}");
    }
    assert!(parse_net_tcp(&[0xff, 0xfe], false).is_err());
    assert_eq!(parse_net_tcp(b"", false), Ok(Vec::new()));
}

#[test]
fn fd_links_give_the_socket_inodes_in_order() {
    let text = String::from_utf8(fixture("proc/fd_targets")).unwrap();
    let targets: Vec<OsString> = text.lines().map(OsString::from).collect();
    assert_eq!(socket_inodes(&targets), [48_715_360, 48_715_361]);
    let odd = [
        "socket:[]",
        "socket:[12",
        "socket:[-1]",
        "anon_inode:[eventpoll]",
    ];
    let odd: Vec<OsString> = odd.into_iter().map(OsString::from).collect();
    assert!(socket_inodes(&odd).is_empty());
}

#[test]
fn meminfo_is_read_in_bytes() {
    let info = parse_meminfo(&fixture("proc/meminfo")).unwrap();
    assert_eq!(info.total, 97_973_900 * 1024);
    assert_eq!(info.available, 93_726_236 * 1024);
    assert_eq!(info.swap_total, 2_097_148 * 1024);
    assert_eq!(info.swap_free, 1_350_412 * 1024);
    let partial = b"MemTotal: 100 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n";
    assert_eq!(
        parse_meminfo(partial),
        Err(ParseError::Malformed("meminfo"))
    );
    let huge =
        b"MemTotal: 99999999999999999999 kB\nMemAvailable: 1 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n";
    assert!(parse_meminfo(huge).is_err());
}

#[test]
fn a_status_gives_the_real_uid_and_the_memory_in_ram_and_swap() {
    let status = parse_status(&fixture("proc/status_self")).unwrap();
    assert_eq!(status.uid, 1000);
    assert_eq!(status.rss_bytes, Some(1968 * 1024));
    assert_eq!(status.swap_bytes, Some(0));
    let kernel_thread = parse_status(&fixture("proc/status_kthread")).unwrap();
    assert_eq!((kernel_thread.uid, kernel_thread.rss_bytes), (0, None));
    assert!(parse_status(b"Name:\tx\nUid:\tnobody\n").is_err());
    assert!(parse_status(b"").is_err());
}

#[test]
fn a_stat_line_gives_the_start_even_with_parentheses_in_the_command() {
    assert_eq!(parse_stat_start(&fixture("proc/stat_self")), Ok(42_760_698));
    let text = String::from_utf8(fixture("proc/stat_self")).unwrap();
    let odd = text.replacen("(cat)", "(a) (b c) d)", 1);
    assert_eq!(parse_stat_start(odd.as_bytes()), Ok(42_760_698));
    assert!(parse_stat_start(b"12 (x) S 1 2").is_err());
    assert!(parse_stat_start(b"no parenthesis").is_err());
}

#[test]
fn mountinfo_gives_devices_mount_points_and_types() {
    let rows = parse_mountinfo(&fixture("proc/mountinfo")).unwrap();
    assert_eq!(rows.len(), 9);
    let root = MountRow::holding(&rows, Path::new("/users/user/.local/share/solar-core")).unwrap();
    assert_eq!((root.major, root.minor), (259, 3));
    assert_eq!(
        (root.mount_point.as_path(), root.fs_type.as_str()),
        (Path::new("/"), "ext4")
    );
    let run = MountRow::holding(&rows, Path::new("/run/user/1000/solar")).unwrap();
    assert_eq!(
        (run.mount_point.as_path(), run.fs_type.as_str()),
        (Path::new("/run/user/1000"), "tmpfs")
    );
    let portal = MountRow::holding(&rows, Path::new("/run/user/1000/doc/x")).unwrap();
    assert_eq!(portal.fs_type, "fuse.portal");

    let remote = parse_mountinfo(&fixture("proc/mountinfo-remote")).unwrap();
    let types: Vec<&str> = remote.iter().map(|row| row.fs_type.as_str()).collect();
    assert_eq!(types, ["nfs4", "cifs", "fuse.sshfs", "tmpfs", "ext4"]);
    assert_eq!(
        remote[3].mount_point,
        PathBuf::from("/users/user/node data")
    );
    assert_eq!((remote[4].major, remote[4].minor), (8, 1));
    let held = MountRow::holding(&remote, Path::new("/users/user/node data/store"));
    assert_eq!(held.map(|row| row.fs_type.as_str()), Some("tmpfs"));
}

#[test]
fn mountinfo_that_is_not_the_kernels_is_refused() {
    for line in [
        "36 35 98:0 /mnt1 /mnt/parent rw,noatime master:1 ext3 /dev/root rw\n",
        "36 35 98 / /mnt rw - ext3 /dev/root rw\n",
        "36 35 x:0 / /mnt rw - ext3 /dev/root rw\n",
        "36 35 98:0 / /mnt rw -\n",
    ] {
        let parsed = parse_mountinfo(line.as_bytes());
        assert_eq!(parsed, Err(ParseError::Malformed("mountinfo")), "{line}");
    }
    let escapes = b"1 2 0:5 / /a\\134b\\011c\\999\\04 rw - tmpfs t rw\n";
    let row = &parse_mountinfo(escapes).unwrap()[0];
    assert_eq!(row.mount_point, PathBuf::from("/a\\b\tc\\999\\04"));
}

#[test]
fn rotational_and_release_files() {
    assert_eq!(
        parse_rotational(&fixture("sys/nvme0n1/queue/rotational")),
        Ok(false)
    );
    assert_eq!(
        parse_rotational(&fixture("sys/sda/queue/rotational")),
        Ok(true)
    );
    assert!(parse_rotational(b"2\n").is_err());
    assert!(parse_rotational(b"").is_err());
    assert_eq!(parse_os_release(b"6.14.0-37-generic\n"), Ok("6.14".into()));
    assert_eq!(parse_os_release(b"5.15-rc1"), Ok("5.15".into()));
    assert!(parse_os_release(b"6\n").is_err());
    assert!(parse_os_release(b"x.y").is_err());
}

/// A world whose `/proc` and `/sys` hold the captured block tree at the paths the view asks for.
fn block_world() -> FakeWorld {
    let mut world = FakeWorld::default();
    let files = [
        (
            ProcFile::BlockRotational {
                major: 259,
                minor: 0,
            },
            "sys/nvme0n1/queue/rotational",
        ),
        (
            ProcFile::BlockPartition {
                major: 259,
                minor: 3,
            },
            "sys/nvme0n1p3/partition",
        ),
        (
            ProcFile::BlockParentRotational {
                major: 259,
                minor: 3,
            },
            "sys/nvme0n1/queue/rotational",
        ),
        (
            ProcFile::BlockRotational { major: 7, minor: 0 },
            "sys/loop0/queue/rotational",
        ),
        (
            ProcFile::BlockPartition { major: 8, minor: 1 },
            "sys/sda1/partition",
        ),
        (
            ProcFile::BlockParentRotational { major: 8, minor: 1 },
            "sys/sda/queue/rotational",
        ),
    ];
    for (file, name) in files {
        world.procfs.files.insert(file.path(), fixture(name));
    }
    world
}

#[test]
fn a_partition_is_judged_by_its_parent_devices_queue() {
    let world = block_world();
    let scope = scope("/nodes/node1");
    let view = Gathered::new(&world, &scope, &DoctorOptions::default());
    let device = |major, minor| view.block_device(major, minor).unwrap();
    let nvme = BlockDevice {
        partition: false,
        rotational: Some(false),
    };
    assert_eq!(device(259, 0), nvme);
    assert_eq!(
        device(259, 3),
        BlockDevice {
            partition: true,
            rotational: Some(false)
        }
    );
    assert_eq!(device(7, 0), nvme);
    assert_eq!(
        device(8, 1),
        BlockDevice {
            partition: true,
            rotational: Some(true)
        }
    );
    let virtual_disk = BlockDevice {
        partition: false,
        rotational: None,
    };
    assert_eq!(device(252, 0), virtual_disk);
}

#[test]
fn the_doctors_mcp_servers_of_one_user_are_counted() {
    assert!(is_doctor_mcp(b"heartwood\0doctor\0mcp\0"));
    assert!(is_doctor_mcp(
        b"/usr/local/bin/heartwood\0doctor:mcp\0--network\0testnet\0"
    ));
    assert!(is_doctor_mcp(b"heartwood\0doctor\0mcp\0--token\0solar\0"));
    assert!(!is_doctor_mcp(b"heartwood\0doctor\0--json\0"));
    assert!(!is_doctor_mcp(b"heartwood\0relay\0start\0"));
    assert!(!is_doctor_mcp(b"/bin/sh\0-c\0heartwood doctor mcp\0"));
    assert!(!is_doctor_mcp(b"xheartwood\0doctor\0mcp\0"));
    assert!(!is_doctor_mcp(b""));

    let mut world = FakeWorld::default();
    let status = |uid: u32| format!("Name:\theartwood\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n");
    let processes = [
        (10, 1000, &b"heartwood\0doctor\0mcp\0"[..]),
        (11, 1000, b"heartwood\0doctor:mcp\0"),
        (12, 1001, b"heartwood\0doctor\0mcp\0"),
        (13, 1000, b"heartwood\0relay\0run\0"),
    ];
    for (pid, uid, cmdline) in processes {
        world.procfs.pids.push(pid);
        world
            .procfs
            .files
            .insert(ProcFile::PidStatus(pid).path(), status(uid).into_bytes());
        world
            .procfs
            .files
            .insert(ProcFile::PidCmdline(pid).path(), cmdline.to_vec());
    }
    world.procfs.pids.push(14);
    assert_eq!(doctor_mcp_servers(&world.procfs, 1000), Ok(2));
    assert_eq!(doctor_mcp_servers(&world.procfs, 1001), Ok(1));
    assert_eq!(doctor_mcp_servers(&world.procfs, 0), Ok(0));
}

#[test]
fn the_host_proc_reads_this_hosts_tables() {
    let proc = HostProc;
    let status = parse_status(&proc.read(ProcFile::SelfStatus).unwrap()).unwrap();
    let pid = std::process::id();
    let own = parse_status(&proc.read(ProcFile::PidStatus(pid)).unwrap()).unwrap();
    assert_eq!(status.uid, own.uid);
    assert!(own.rss_bytes.is_some_and(|rss| rss > 0));
    assert!(parse_stat_start(&proc.read(ProcFile::PidStat(pid)).unwrap()).is_ok());
    assert!(parse_meminfo(&proc.read(ProcFile::Meminfo).unwrap()).is_ok_and(|m| m.total > 0));
    assert!(parse_os_release(&proc.read(ProcFile::OsRelease).unwrap()).is_ok());
    let mounts = parse_mountinfo(&proc.read(ProcFile::SelfMountinfo).unwrap()).unwrap();
    assert!(MountRow::holding(&mounts, Path::new("/")).is_some());
    let pids = proc.pids().unwrap();
    assert!(pids.contains(&pid));
    assert!(pids.windows(2).all(|pair| pair[0] < pair[1]));
    let cmdline = proc.read(ProcFile::PidCmdline(pid)).unwrap();
    assert!(!is_doctor_mcp(&cmdline));
    let gone = proc.read(ProcFile::PidStatus(u32::MAX));
    assert_eq!(gone, Err(FsError::Io(std::io::ErrorKind::NotFound)));
}

#[test]
fn the_host_proc_finds_a_listening_socket_by_its_descriptors_inode() {
    let block = std::env::var("HEARTWOOD_TEST_LOOPBACK").unwrap_or_else(|_| "127.0.0".into());
    let listener = TcpListener::bind(format!("{block}.1:0")).unwrap();
    let local = listener.local_addr().unwrap();
    let proc = HostProc;
    let inodes = socket_inodes(&proc.fd_targets(std::process::id()).unwrap());
    let rows = parse_net_tcp(&proc.read(ProcFile::NetTcp).unwrap(), false).unwrap();
    let row = rows.iter().find(|row| row.local == local).unwrap();
    assert_eq!(row.state, TcpState::Listen);
    assert!(inodes.contains(&row.inode), "{inodes:?}");
    let own = parse_status(&proc.read(ProcFile::SelfStatus).unwrap()).unwrap();
    assert_eq!(row.uid, own.uid);
    assert!(parse_net_tcp(&proc.read(ProcFile::NetTcp6).unwrap(), true).is_ok());
}

#[test]
fn the_view_reads_this_hosts_root_device_and_own_user() {
    let node = doctor_support::FakeNode::default();
    let scope = scope("/nodes/node1");
    let world = HostWorld::new(&scope, &node);
    let view = Gathered::new(&world, &scope, &DoctorOptions::default());
    let mounts = view.mounts().unwrap();
    let root = MountRow::holding(mounts, Path::new("/")).unwrap();
    let device = view.block_device(root.major, root.minor);
    assert!(device.is_ok(), "{device:?}");
    assert!(view.own_uid().is_ok());
    assert!(view.meminfo().is_ok());
    let unknown = view.role_status(heartwood_cli::Role::Relay).unwrap_err();
    assert_eq!(unknown.reason, Reason::NotRunning);
}
