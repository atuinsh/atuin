pub type shmatt_t = ::c_ulong;
pub type msgqnum_t = ::c_ulong;
pub type msglen_t = ::c_ulong;
pub type regoff_t = ::c_int;

s! {
    pub struct statvfs {  // Different than GNU!
        pub f_bsize: ::c_ulong,
        pub f_frsize: ::c_ulong,
        pub f_blocks: ::fsblkcnt_t,
        pub f_bfree: ::fsblkcnt_t,
        pub f_bavail: ::fsblkcnt_t,
        pub f_files: ::fsfilcnt_t,
        pub f_ffree: ::fsfilcnt_t,
        pub f_favail: ::fsfilcnt_t,
        #[cfg(target_endian = "little")]
        pub f_fsid: ::c_ulong,
        #[cfg(target_pointer_width = "32")]
        __f_unused: ::c_int,
        #[cfg(target_endian = "big")]
        pub f_fsid: ::c_ulong,
        pub f_flag: ::c_ulong,
        pub f_namemax: ::c_ulong,
        __f_spare: [::c_int; 6],
    }

    pub struct regex_t {
        __buffer: *mut ::c_void,
        __allocated: ::size_t,
        __used: ::size_t,
        __syntax: ::c_ulong,
        __fastmap: *mut ::c_char,
        __translate: *mut ::c_char,
        __re_nsub: ::size_t,
        __bitfield: u8,
    }
}

pub const MCL_CURRENT: ::c_int = 0x0001;
pub const MCL_FUTURE: ::c_int = 0x0002;

pub const SIGEV_THREAD_ID: ::c_int = 4;

pub const ADFS_SUPER_MAGIC: ::c_long = 0x0000adf5;
pub const AFFS_SUPER_MAGIC: ::c_long = 0x0000adff;
pub const AFS_SUPER_MAGIC: ::c_long = 0x5346414f;
pub const AUTOFS_SUPER_MAGIC: ::c_long = 0x0187;
pub const BINDERFS_SUPER_MAGIC: ::c_long = 0x6c6f6f70;
pub const BPF_FS_MAGIC: ::c_long = 0xcafe4a11;
pub const BTRFS_SUPER_MAGIC: ::c_long = 0x9123683e;
pub const CGROUP2_SUPER_MAGIC: ::c_long = 0x63677270;
pub const CGROUP_SUPER_MAGIC: ::c_long = 0x27e0eb;
pub const CODA_SUPER_MAGIC: ::c_long = 0x73757245;
pub const CRAMFS_MAGIC: ::c_long = 0x28cd3d45;
pub const DEBUGFS_MAGIC: ::c_long = 0x64626720;
pub const DEVPTS_SUPER_MAGIC: ::c_long = 0x1cd1;
pub const ECRYPTFS_SUPER_MAGIC: ::c_long = 0xf15f;
pub const EFS_SUPER_MAGIC: ::c_long = 0x00414a53;
pub const EXT2_SUPER_MAGIC: ::c_long = 0x0000ef53;
pub const EXT3_SUPER_MAGIC: ::c_long = 0x0000ef53;
pub const EXT4_SUPER_MAGIC: ::c_long = 0x0000ef53;
pub const F2FS_SUPER_MAGIC: ::c_long = 0xf2f52010;
pub const FUTEXFS_SUPER_MAGIC: ::c_long = 0xbad1dea;
pub const HOSTFS_SUPER_MAGIC: ::c_long = 0x00c0ffee;
pub const HPFS_SUPER_MAGIC: ::c_long = 0xf995e849;
pub const HUGETLBFS_MAGIC: ::c_long = 0x958458f6;
pub const ISOFS_SUPER_MAGIC: ::c_long = 0x00009660;
pub const JFFS2_SUPER_MAGIC: ::c_long = 0x000072b6;
pub const MINIX2_SUPER_MAGIC2: ::c_long = 0x00002478;
pub const MINIX2_SUPER_MAGIC: ::c_long = 0x00002468;
pub const MINIX3_SUPER_MAGIC: ::c_long = 0x4d5a;
pub const MINIX_SUPER_MAGIC2: ::c_long = 0x0000138f;
pub const MINIX_SUPER_MAGIC: ::c_long = 0x0000137f;
pub const MSDOS_SUPER_MAGIC: ::c_long = 0x00004d44;
pub const NCP_SUPER_MAGIC: ::c_long = 0x0000564c;
pub const NFS_SUPER_MAGIC: ::c_long = 0x00006969;
pub const NILFS_SUPER_MAGIC: ::c_long = 0x3434;
pub const OCFS2_SUPER_MAGIC: ::c_long = 0x7461636f;
pub const OPENPROM_SUPER_MAGIC: ::c_long = 0x00009fa1;
pub const OVERLAYFS_SUPER_MAGIC: ::c_long = 0x794c7630;
pub const PROC_SUPER_MAGIC: ::c_long = 0x00009fa0;
pub const QNX4_SUPER_MAGIC: ::c_long = 0x0000002f;
pub const QNX6_SUPER_MAGIC: ::c_long = 0x68191122;
pub const RDTGROUP_SUPER_MAGIC: ::c_long = 0x7655821;
pub const REISERFS_SUPER_MAGIC: ::c_long = 0x52654973;
pub const SMB_SUPER_MAGIC: ::c_long = 0x0000517b;
pub const SYSFS_MAGIC: ::c_long = 0x62656572;
pub const TMPFS_MAGIC: ::c_long = 0x01021994;
pub const TRACEFS_MAGIC: ::c_long = 0x74726163;
pub const UDF_SUPER_MAGIC: ::c_long = 0x15013346;
pub const USBDEVICE_SUPER_MAGIC: ::c_long = 0x00009fa2;
pub const XENFS_SUPER_MAGIC: ::c_long = 0xabba1974;
pub const XFS_SUPER_MAGIC: ::c_long = 0x58465342;

pub const PTRACE_TRACEME: ::c_int = 0;
pub const PTRACE_PEEKTEXT: ::c_int = 1;
pub const PTRACE_PEEKDATA: ::c_int = 2;
pub const PTRACE_PEEKUSER: ::c_int = 3;
pub const PTRACE_POKETEXT: ::c_int = 4;
pub const PTRACE_POKEDATA: ::c_int = 5;
pub const PTRACE_POKEUSER: ::c_int = 6;
pub const PTRACE_CONT: ::c_int = 7;
pub const PTRACE_KILL: ::c_int = 8;
pub const PTRACE_SINGLESTEP: ::c_int = 9;
pub const PTRACE_GETREGS: ::c_int = 12;
pub const PTRACE_SETREGS: ::c_int = 13;
pub const PTRACE_GETFPREGS: ::c_int = 14;
pub const PTRACE_SETFPREGS: ::c_int = 15;
pub const PTRACE_ATTACH: ::c_int = 16;
pub const PTRACE_DETACH: ::c_int = 17;
pub const PTRACE_GETFPXREGS: ::c_int = 18;
pub const PTRACE_SETFPXREGS: ::c_int = 19;
pub const PTRACE_SYSCALL: ::c_int = 24;
pub const PTRACE_SETOPTIONS: ::c_int = 0x4200;
pub const PTRACE_GETEVENTMSG: ::c_int = 0x4201;
pub const PTRACE_GETSIGINFO: ::c_int = 0x4202;
pub const PTRACE_SETSIGINFO: ::c_int = 0x4203;
pub const PTRACE_GETREGSET: ::c_int = 0x4204;
pub const PTRACE_SETREGSET: ::c_int = 0x4205;
pub const PTRACE_SEIZE: ::c_int = 0x4206;
pub const PTRACE_INTERRUPT: ::c_int = 0x4207;
pub const PTRACE_LISTEN: ::c_int = 0x4208;
pub const PTRACE_O_MASK: ::c_int = 0x000000ff;

pub const POSIX_FADV_DONTNEED: ::c_int = 4;
pub const POSIX_FADV_NOREUSE: ::c_int = 5;

pub const RLIMIT_CPU: ::c_int = 0;
pub const RLIMIT_FSIZE: ::c_int = 1;
pub const RLIMIT_DATA: ::c_int = 2;
pub const RLIMIT_STACK: ::c_int = 3;
pub const RLIMIT_CORE: ::c_int = 4;
pub const RLIMIT_LOCKS: ::c_int = 10;
pub const RLIMIT_SIGPENDING: ::c_int = 11;
pub const RLIMIT_MSGQUEUE: ::c_int = 12;
pub const RLIMIT_NICE: ::c_int = 13;
pub const RLIMIT_RTPRIO: ::c_int = 14;

// These are different than GNU!
pub const LC_CTYPE: ::c_int = 0;
pub const LC_NUMERIC: ::c_int = 1;
pub const LC_TIME: ::c_int = 3;
pub const LC_COLLATE: ::c_int = 4;
pub const LC_MONETARY: ::c_int = 2;
pub const LC_MESSAGES: ::c_int = 5;
pub const LC_ALL: ::c_int = 6;
// end different section

// MS_ flags for mount(2)
pub const MS_RMT_MASK: ::c_ulong = ::MS_RDONLY | ::MS_SYNCHRONOUS | ::MS_MANDLOCK | ::MS_I_VERSION;

pub const ENOTSUP: ::c_int = EOPNOTSUPP;

pub const IPV6_JOIN_GROUP: ::c_int = 20;
pub const IPV6_LEAVE_GROUP: ::c_int = 21;

// These are different from GNU
pub const ABDAY_1: ::nl_item = 0x300;
pub const ABDAY_2: ::nl_item = 0x301;
pub const ABDAY_3: ::nl_item = 0x302;
pub const ABDAY_4: ::nl_item = 0x303;
pub const ABDAY_5: ::nl_item = 0x304;
pub const ABDAY_6: ::nl_item = 0x305;
pub const ABDAY_7: ::nl_item = 0x306;
pub const DAY_1: ::nl_item = 0x307;
pub const DAY_2: ::nl_item = 0x308;
pub const DAY_3: ::nl_item = 0x309;
pub const DAY_4: ::nl_item = 0x30A;
pub const DAY_5: ::nl_item = 0x30B;
pub const DAY_6: ::nl_item = 0x30C;
pub const DAY_7: ::nl_item = 0x30D;
pub const ABMON_1: ::nl_item = 0x30E;
pub const ABMON_2: ::nl_item = 0x30F;
pub const ABMON_3: ::nl_item = 0x310;
pub const ABMON_4: ::nl_item = 0x311;
pub const ABMON_5: ::nl_item = 0x312;
pub const ABMON_6: ::nl_item = 0x313;
pub const ABMON_7: ::nl_item = 0x314;
pub const ABMON_8: ::nl_item = 0x315;
pub const ABMON_9: ::nl_item = 0x316;
pub const ABMON_10: ::nl_item = 0x317;
pub const ABMON_11: ::nl_item = 0x318;
pub const ABMON_12: ::nl_item = 0x319;
pub const MON_1: ::nl_item = 0x31A;
pub const MON_2: ::nl_item = 0x31B;
pub const MON_3: ::nl_item = 0x31C;
pub const MON_4: ::nl_item = 0x31D;
pub const MON_5: ::nl_item = 0x31E;
pub const MON_6: ::nl_item = 0x31F;
pub const MON_7: ::nl_item = 0x320;
pub const MON_8: ::nl_item = 0x321;
pub const MON_9: ::nl_item = 0x322;
pub const MON_10: ::nl_item = 0x323;
pub const MON_11: ::nl_item = 0x324;
pub const MON_12: ::nl_item = 0x325;
pub const AM_STR: ::nl_item = 0x326;
pub const PM_STR: ::nl_item = 0x327;
pub const D_T_FMT: ::nl_item = 0x328;
pub const D_FMT: ::nl_item = 0x329;
pub const T_FMT: ::nl_item = 0x32A;
pub const T_FMT_AMPM: ::nl_item = 0x32B;
pub const ERA: ::nl_item = 0x32C;
pub const ERA_D_FMT: ::nl_item = 0x32E;
pub const ALT_DIGITS: ::nl_item = 0x32F;
pub const ERA_D_T_FMT: ::nl_item = 0x330;
pub const ERA_T_FMT: ::nl_item = 0x331;
pub const CODESET: ::nl_item = 10;
pub const CRNCYSTR: ::nl_item = 0x215;
pub const RADIXCHAR: ::nl_item = 0x100;
pub const THOUSEP: ::nl_item = 0x101;
pub const NOEXPR: ::nl_item = 0x501;
pub const YESSTR: ::nl_item = 0x502;
pub const NOSTR: ::nl_item = 0x503;

// Different than Gnu.
pub const FILENAME_MAX: ::c_uint = 4095;

pub const PRIO_PROCESS: ::c_int = 0;
pub const PRIO_PGRP: ::c_int = 1;
pub const PRIO_USER: ::c_int = 2;

pub const ST_RELATIME: ::c_ulong = 4096;

extern "C" {
    pub fn gettimeofday(tp: *mut ::timeval, tz: *mut ::timezone) -> ::c_int;

    pub fn pthread_rwlockattr_getkind_np(
        attr: *const ::pthread_rwlockattr_t,
        val: *mut ::c_int,
    ) -> ::c_int;
    pub fn pthread_rwlockattr_setkind_np(
        attr: *mut ::pthread_rwlockattr_t,
        val: ::c_int,
    ) -> ::c_int;

    pub fn ptrace(request: ::c_uint, ...) -> ::c_long;

    pub fn sendmmsg(
        sockfd: ::c_int,
        msgvec: *mut ::mmsghdr,
        vlen: ::c_uint,
        flags: ::c_int,
    ) -> ::c_int;
    pub fn recvmmsg(
        sockfd: ::c_int,
        msgvec: *mut ::mmsghdr,
        vlen: ::c_uint,
        flags: ::c_int,
        timeout: *mut ::timespec,
    ) -> ::c_int;

    pub fn openpty(
        amaster: *mut ::c_int,
        aslave: *mut ::c_int,
        name: *mut ::c_char,
        termp: *mut termios,
        winp: *mut ::winsize,
    ) -> ::c_int;
    pub fn forkpty(
        amaster: *mut ::c_int,
        name: *mut ::c_char,
        termp: *mut termios,
        winp: *mut ::winsize,
    ) -> ::pid_t;

    pub fn getnameinfo(
        sa: *const ::sockaddr,
        salen: ::socklen_t,
        host: *mut ::c_char,
        hostlen: ::socklen_t,
        serv: *mut ::c_char,
        sevlen: ::socklen_t,
        flags: ::c_int,
    ) -> ::c_int;

    pub fn pwritev(
        fd: ::c_int,
        iov: *const ::iovec,
        iovcnt: ::c_int,
        offset: ::off64_t,
    ) -> ::ssize_t;
    pub fn preadv(
        fd: ::c_int,
        iov: *const ::iovec,
        iovcnt: ::c_int,
        offset: ::off64_t,
    ) -> ::ssize_t;
}

cfg_if! {
    if #[cfg(any(target_arch = "mips", target_arch = "mips64"))] {
        mod mips;
        pub use self::mips::*;
    } else if #[cfg(target_arch = "x86_64")] {
        mod x86_64;
        pub use self::x86_64::*;
    } else if #[cfg(target_arch = "arm")] {
        mod arm;
        pub use self::arm::*;
    } else {
        pub use unsupported_target;
    }
}
