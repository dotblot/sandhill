use super::*;

#[cfg(target_os = "linux")]
pub(super) fn process(pid: u32) -> Option<Process> {
    use std::os::unix::fs::MetadataExt;
    let path = format!("/proc/{pid}");
    let uid = std::fs::metadata(&path).ok()?.uid();
    let text = std::fs::read_to_string(format!("{path}/stat")).ok()?;
    let fields: Vec<&str> = text
        .get(text.rfind(')')? + 2..)?
        .split_whitespace()
        .collect();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(Process {
        identity: ProcessIdentity {
            pid,
            uid,
            birth: format!("{}:{}", boot.trim(), fields.get(19)?),
        },
        parent: fields.get(1)?.parse().ok()?,
        group: fields.get(2)?.parse().ok()?,
        session: Some(fields.get(3)?.parse().ok()?),
        zombie: *fields.first()? == "Z",
    })
}

#[cfg(target_os = "macos")]
pub(super) fn process(pid: u32) -> Option<Process> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            1, // PROC_PIDTBSDINFO arg != 0 explicitly includes zombies.
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(Process {
        identity: ProcessIdentity {
            pid,
            uid: info.pbi_uid,
            birth: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        },
        parent: info.pbi_ppid,
        group: info.pbi_pgid,
        // getsid can return ESRCH for a zombie or a process transitioning
        // through exit, even while BSD identity/status remain readable.
        // A missing session must not erase that independent exit evidence.
        session: unsafe { libc::getsid(pid as i32) }.try_into().ok(),
        zombie: info.pbi_status == 5,
    })
}

pub(super) fn processes() -> Result<HashMap<u32, Process>> {
    #[cfg(target_os = "linux")]
    let pids: Vec<u32> = std::fs::read_dir("/proc")?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    #[cfg(target_os = "macos")]
    let pids: Vec<u32> = {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            bail!("cannot enumerate processes");
        }
        let mut pids = vec![0i32; count as usize + 1024];
        let size = (pids.len() * std::mem::size_of::<i32>()) as i32;
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
        if n <= 0 || n as usize >= pids.len() {
            bail!("process enumeration was incomplete");
        }
        pids.truncate(n as usize);
        pids.into_iter().filter_map(|p| p.try_into().ok()).collect()
    };
    Ok(pids
        .into_iter()
        .filter_map(|pid| process(pid).map(|p| (pid, p)))
        .collect())
}

/// Capture children before sending any shutdown request. Kept identities also
/// account for descendants that outlive their original kernel or terminal host.
pub(super) fn refresh(record: &mut Ownership, all: &mut HashMap<u32, Process>) -> Result<bool> {
    for identity in record.roots.iter().chain(&record.members) {
        // Enumeration and per-process inspection are not an atomic OS
        // snapshot. Resolve known PIDs directly before treating an omitted
        // process as unreadable. This also retains readable zombie status.
        if let std::collections::hash_map::Entry::Vacant(entry) = all.entry(identity.pid) {
            if let Some(current) = process(identity.pid) {
                entry.insert(current);
            } else if crate::kernel::pid_alive(identity.pid) {
                bail!(
                    "cannot inspect live recorded process {}; refusing to assume it stopped",
                    identity.pid
                );
            }
        }
    }
    refresh_with_liveness(record, all, ProcessIdentity::alive)
}

fn refresh_with_liveness(
    record: &mut Ownership,
    all: &HashMap<u32, Process>,
    alive: impl Fn(&ProcessIdentity) -> bool,
) -> Result<bool> {
    let original_len = record.members.len();
    record.members.retain(|id| {
        all.get(&id.pid)
            .is_some_and(|p| p.identity == *id && !p.zombie)
            && alive(id)
    });
    let mut owned: HashSet<u32> = record.members.iter().map(|p| p.pid).collect();
    for root in &record.roots {
        if all
            .get(&root.pid)
            .is_some_and(|p| p.identity == *root && !p.zombie)
            && alive(root)
        {
            owned.insert(root.pid);
        }
    }
    loop {
        let mut added = false;
        for p in all.values() {
            if p.zombie
                || p.identity.uid != unsafe { libc::getuid() }
                || owned.contains(&p.identity.pid)
            {
                continue;
            }
            let grouped = record.roots.iter().any(|root| {
                (owned.contains(&root.pid)
                    && all
                        .get(&root.pid)
                        .is_some_and(|live| live.identity == *root && !live.zombie)
                    || owned.iter().any(|pid| {
                        all.get(pid).is_some_and(|live| {
                            live.group == root.pid || live.session == Some(root.pid)
                        })
                    }))
                    && (p.group == root.pid || p.session == Some(root.pid))
            });
            // Confirm candidate birth identity after the whole snapshot was
            // collected, before it may establish ancestry for another PID.
            if (grouped || owned.contains(&p.parent)) && alive(&p.identity) {
                owned.insert(p.identity.pid);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let mut members: Vec<_> = owned
        .into_iter()
        .filter_map(|pid| all.get(&pid).map(|p| p.identity.clone()))
        .collect();
    members.sort_by_key(|p| p.pid);
    let changed = original_len != record.members.len() || members != record.members;
    record.members = members;
    // A group may have outlived its leader before the first inspection. Do
    // not lose it from inventory, but do not manufacture signal ownership:
    // a historical group number alone could have been reused.
    let mut unverified: Vec<_> = all
        .values()
        .filter(|p| {
            !p.zombie
                && p.identity.uid == unsafe { libc::getuid() }
                && !record.members.contains(&p.identity)
                && record
                    .roots
                    .iter()
                    .any(|root| p.group == root.pid || p.session == Some(root.pid))
        })
        .map(|p| p.identity.clone())
        .collect();
    unverified.sort_by_key(|p| p.pid);
    let changed = changed || unverified != record.unverified;
    record.unverified = unverified;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_pid_does_not_match_owned_process() {
        let live = ProcessIdentity::read(std::process::id()).unwrap();
        let mut stale = live.clone();
        stale.birth.push_str("-old");
        assert!(live.alive());
        assert!(!stale.alive());
        let mut ownership = Ownership {
            roots: vec![stale.clone()],
            members: vec![stale],
            unverified: Vec::new(),
        };
        refresh(&mut ownership, &mut processes().unwrap()).unwrap();
        assert!(ownership.members.is_empty());
    }
    #[test]
    fn known_orphans_remain_owned_but_unobserved_groups_are_only_reported() {
        let root = ProcessIdentity {
            pid: 100,
            uid: unsafe { libc::getuid() },
            birth: "root".into(),
        };
        let child = ProcessIdentity {
            pid: 101,
            uid: root.uid,
            birth: "child".into(),
        };
        let grandchild = ProcessIdentity {
            pid: 102,
            uid: root.uid,
            birth: "grandchild".into(),
        };
        let all = HashMap::from([
            (
                100,
                Process {
                    identity: root.clone(),
                    parent: 1,
                    group: 100,
                    session: Some(100),
                    zombie: true,
                },
            ),
            (
                101,
                Process {
                    identity: child.clone(),
                    parent: 1,
                    group: 100,
                    session: Some(100),
                    zombie: false,
                },
            ),
            (
                102,
                Process {
                    identity: grandchild.clone(),
                    parent: 1,
                    group: 100,
                    session: Some(100),
                    zombie: false,
                },
            ),
        ]);
        let mut known = Ownership {
            roots: vec![root.clone()],
            members: vec![child],
            unverified: vec![],
        };
        refresh_with_liveness(&mut known, &all, |_| true).unwrap();
        assert!(known.members.contains(&grandchild));
        assert!(known.unverified.is_empty());
        let mut unknown = Ownership {
            roots: vec![root],
            members: vec![],
            unverified: vec![],
        };
        refresh_with_liveness(&mut unknown, &all, |_| true).unwrap();
        assert!(unknown.members.is_empty());
        assert_eq!(unknown.unverified.len(), 2);
    }

    #[test]
    fn recorded_live_process_omitted_from_enumeration_is_inspected_directly() {
        let identity = ProcessIdentity::read(std::process::id()).unwrap();
        let mut record = Ownership {
            roots: vec![identity.clone()],
            members: vec![identity.clone()],
            unverified: Vec::new(),
        };
        let mut snapshot = HashMap::new();
        refresh(&mut record, &mut snapshot).unwrap();
        assert_eq!(snapshot[&identity.pid].identity, identity);
        assert!(record.members.contains(&identity));
    }

    #[test]
    fn unreaped_exit_is_not_an_unreadable_live_resource() {
        use std::io::Write;
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "read line"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let identity = ProcessIdentity::read(child.id()).unwrap();
        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        let mut status = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        // WNOWAIT observes exit without reaping: kill(pid, 0) still succeeds.
        let waited = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                status.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        let observed = process(child.id());
        let existed_before_reap = crate::kernel::pid_alive(child.id());
        let mut record = Ownership {
            roots: vec![identity.clone()],
            members: vec![identity],
            unverified: Vec::new(),
        };
        let inspected = refresh(&mut record, &mut HashMap::new());
        // Reap our own child before any assertions, including failure paths.
        let exited = child.wait().unwrap();
        assert_eq!(waited, 0);
        assert!(exited.success());
        assert!(
            existed_before_reap,
            "the regression requires an existing unreaped PID"
        );
        assert!(observed.is_some_and(|process| process.zombie));
        assert!(inspected.is_ok(), "{inspected:?}");
        assert!(record.members.is_empty());
    }
}
