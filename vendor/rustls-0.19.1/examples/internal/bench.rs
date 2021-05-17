// This program does assorted benchmarking of rustls.
//
// Note: we don't use any of the standard 'cargo bench', 'test::Bencher',
// etc. because it's unstable at the time of writing.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls;
use rustls::internal::pemfile;
use rustls::ClientSessionMemoryCache;
use rustls::NoClientSessionStorage;
use rustls::NoServerSessionStorage;
use rustls::ProtocolVersion;
use rustls::ServerSessionMemoryCache;
use rustls::Session;
use rustls::Ticketer;
use rustls::{AllowAnyAuthenticatedClient, NoClientAuth, RootCertStore};
use rustls::{ClientConfig, ClientSession};
use rustls::{ServerConfig, ServerSession};

use webpki;

fn duration_nanos(d: Duration) -> f64 {
    (d.as_secs() as f64) + f64::from(d.subsec_nanos()) / 1e9
}

fn _bench<Fsetup, Ftest, S>(count: usize, name: &'static str, f_setup: Fsetup, f_test: Ftest)
where
    Fsetup: Fn() -> S,
    Ftest: Fn(S),
{
    let mut times = Vec::new();

    for _ in 0..count {
        let state = f_setup();
        let start = Instant::now();
        f_test(state);
        times.push(duration_nanos(Instant::now().duration_since(start)));
    }

    println!("{}", name);
    println!("{:?}", times);
}

fn time<F>(mut f: F) -> f64
where
    F: FnMut(),
{
    let start = Instant::now();
    f();
    let end = Instant::now();
    let dur = duration_nanos(end.duration_since(start));
    f64::from(dur)
}

fn transfer(left: &mut dyn Session, right: &mut dyn Session) -> f64 {
    let mut buf = [0u8; 262144];
    let mut read_time = 0f64;

    loop {
        let mut sz = 0;

        while left.wants_write() {
            let written = left
                .write_tls(&mut buf[sz..].as_mut())
                .unwrap();
            if written == 0 {
                break;
            }

            sz += written;
        }

        if sz == 0 {
            return read_time;
        }

        let mut offs = 0;
        loop {
            let start = Instant::now();
            offs += right
                .read_tls(&mut buf[offs..sz].as_ref())
                .unwrap();
            let end = Instant::now();
            read_time += f64::from(duration_nanos(end.duration_since(start)));
            if sz == offs {
                break;
            }
        }
    }
}

fn drain(d: &mut dyn Session, expect_len: usize) {
    let mut left = expect_len;
    let mut buf = [0u8; 8192];
    loop {
        let sz = d.read(&mut buf).unwrap();
        left -= sz;
        if left == 0 {
            break;
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum ClientAuth {
    No,
    Yes,
}

#[derive(PartialEq, Clone, Copy)]
enum Resumption {
    No,
    SessionID,
    Tickets,
}

impl Resumption {
    fn label(&self) -> &'static str {
        match *self {
            Resumption::No => "no-resume",
            Resumption::SessionID => "sessionid",
            Resumption::Tickets => "tickets",
        }
    }
}

// copied from tests/api.rs
#[derive(PartialEq, Clone, Copy, Debug)]
enum KeyType {
    RSA,
    ECDSA,
    ED25519,
}

struct BenchmarkParam {
    key_type: KeyType,
    ciphersuite: &'static rustls::SupportedCipherSuite,
    version: ProtocolVersion,
}

impl BenchmarkParam {
    const fn new(
        key_type: KeyType,
        ciphersuite: &'static rustls::SupportedCipherSuite,
        version: ProtocolVersion,
    ) -> BenchmarkParam {
        BenchmarkParam {
            key_type,
            ciphersuite,
            version,
        }
    }
}

static ALL_BENCHMARKS: &[BenchmarkParam] = &[
    BenchmarkParam::new(
        KeyType::RSA,
        &rustls::ciphersuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        ProtocolVersion::TLSv1_2,
    ),
    BenchmarkParam::new(
        KeyType::ECDSA,
        &rustls::ciphersuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        ProtocolVersion::TLSv1_2,
    ),
    BenchmarkParam::new(
        KeyType::RSA,
        &rustls::ciphersuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        ProtocolVersion::TLSv1_2,
    ),
    BenchmarkParam::new(
        KeyType::RSA,
        &rustls::ciphersuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        ProtocolVersion::TLSv1_2,
    ),
    BenchmarkParam::new(
        KeyType::RSA,
        &rustls::ciphersuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        ProtocolVersion::TLSv1_2,
    ),
    BenchmarkParam::new(
        KeyType::ECDSA,
        &rustls::ciphersuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        ProtocolVersion::TLSv1_2,
    ),
    BenchmarkParam::new(
        KeyType::ECDSA,
        &rustls::ciphersuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        ProtocolVersion::TLSv1_2,
    ),
    BenchmarkParam::new(
        KeyType::RSA,
        &rustls::ciphersuite::TLS13_CHACHA20_POLY1305_SHA256,
        ProtocolVersion::TLSv1_3,
    ),
    BenchmarkParam::new(
        KeyType::RSA,
        &rustls::ciphersuite::TLS13_AES_256_GCM_SHA384,
        ProtocolVersion::TLSv1_3,
    ),
    BenchmarkParam::new(
        KeyType::RSA,
        &rustls::ciphersuite::TLS13_AES_128_GCM_SHA256,
        ProtocolVersion::TLSv1_3,
    ),
    BenchmarkParam::new(
        KeyType::ECDSA,
        &rustls::ciphersuite::TLS13_AES_128_GCM_SHA256,
        ProtocolVersion::TLSv1_3,
    ),
    BenchmarkParam::new(
        KeyType::ED25519,
        &rustls::ciphersuite::TLS13_AES_128_GCM_SHA256,
        ProtocolVersion::TLSv1_3,
    ),
];

impl KeyType {
    fn path_for(&self, part: &str) -> String {
        match self {
            KeyType::RSA => format!("test-ca/rsa/{}", part),
            KeyType::ECDSA => format!("test-ca/ecdsa/{}", part),
            KeyType::ED25519 => format!("test-ca/eddsa/{}", part),
        }
    }

    fn get_chain(&self) -> Vec<rustls::Certificate> {
        pemfile::certs(&mut io::BufReader::new(
            fs::File::open(self.path_for("end.fullchain")).unwrap(),
        ))
        .unwrap()
    }

    fn get_key(&self) -> rustls::PrivateKey {
        pemfile::pkcs8_private_keys(&mut io::BufReader::new(
            fs::File::open(self.path_for("end.key")).unwrap(),
        ))
        .unwrap()[0]
            .clone()
    }

    fn get_client_chain(&self) -> Vec<rustls::Certificate> {
        pemfile::certs(&mut io::BufReader::new(
            fs::File::open(self.path_for("client.fullchain")).unwrap(),
        ))
        .unwrap()
    }

    fn get_client_key(&self) -> rustls::PrivateKey {
        pemfile::pkcs8_private_keys(&mut io::BufReader::new(
            fs::File::open(self.path_for("client.key")).unwrap(),
        ))
        .unwrap()[0]
            .clone()
    }
}

fn make_server_config(
    params: &BenchmarkParam,
    client_auth: ClientAuth,
    resume: Resumption,
    mtu: Option<usize>,
) -> ServerConfig {
    let client_auth = match client_auth {
        ClientAuth::Yes => {
            let roots = params.key_type.get_chain();
            let mut client_auth_roots = RootCertStore::empty();
            for root in roots {
                client_auth_roots.add(&root).unwrap();
            }
            AllowAnyAuthenticatedClient::new(client_auth_roots)
        }
        ClientAuth::No => NoClientAuth::new(),
    };

    let mut cfg = ServerConfig::new(client_auth);
    cfg.set_single_cert(params.key_type.get_chain(), params.key_type.get_key())
        .expect("bad certs/private key?");

    if resume == Resumption::SessionID {
        cfg.set_persistence(ServerSessionMemoryCache::new(128));
    } else if resume == Resumption::Tickets {
        cfg.ticketer = Ticketer::new();
    } else {
        cfg.set_persistence(Arc::new(NoServerSessionStorage {}));
    }

    cfg.versions.clear();
    cfg.versions.push(params.version);

    cfg.mtu = mtu;

    cfg
}

fn make_client_config(
    params: &BenchmarkParam,
    clientauth: ClientAuth,
    resume: Resumption,
) -> ClientConfig {
    let mut cfg = ClientConfig::new();
    let mut rootbuf =
        io::BufReader::new(fs::File::open(params.key_type.path_for("ca.cert")).unwrap());
    cfg.root_store
        .add_pem_file(&mut rootbuf)
        .unwrap();
    cfg.ciphersuites.clear();
    cfg.ciphersuites
        .push(params.ciphersuite);
    cfg.versions.clear();
    cfg.versions.push(params.version);

    if clientauth == ClientAuth::Yes {
        cfg.set_single_client_cert(
            params.key_type.get_client_chain(),
            params.key_type.get_client_key(),
        )
        .unwrap();
    }

    if resume != Resumption::No {
        cfg.set_persistence(ClientSessionMemoryCache::new(128));
    } else {
        cfg.set_persistence(Arc::new(NoClientSessionStorage {}));
    }

    cfg
}

fn apply_work_multiplier(work: u64) -> u64 {
    let mul = match env::var("BENCH_MULTIPLIER") {
        Ok(val) => val
            .parse::<f64>()
            .expect("invalid BENCH_MULTIPLIER value"),
        Err(_) => 1.,
    };

    ((work as f64) * mul).round() as u64
}

fn bench_handshake(params: &BenchmarkParam, clientauth: ClientAuth, resume: Resumption) {
    let client_config = Arc::new(make_client_config(params, clientauth, resume));
    let server_config = Arc::new(make_server_config(params, clientauth, resume, None));

    assert!(
        params
            .ciphersuite
            .usable_for_version(params.version)
    );

    let rounds = apply_work_multiplier(if resume == Resumption::No { 512 } else { 4096 });
    let mut client_time = 0f64;
    let mut server_time = 0f64;

    for _ in 0..rounds {
        let dns_name = webpki::DNSNameRef::try_from_ascii_str("localhost").unwrap();
        let mut client = ClientSession::new(&client_config, dns_name);
        let mut server = ServerSession::new(&server_config);

        server_time += time(|| {
            transfer(&mut client, &mut server);
            server.process_new_packets().unwrap()
        });
        client_time += time(|| {
            transfer(&mut server, &mut client);
            client.process_new_packets().unwrap()
        });
        server_time += time(|| {
            transfer(&mut client, &mut server);
            server.process_new_packets().unwrap()
        });
        client_time += time(|| {
            transfer(&mut server, &mut client);
            client.process_new_packets().unwrap()
        });
    }

    println!(
        "handshakes\t{:?}\t{:?}\t{:?}\tclient\t{}\t{}\t{:.2}\thandshake/s",
        params.version,
        params.key_type,
        params.ciphersuite.suite,
        if clientauth == ClientAuth::Yes {
            "mutual"
        } else {
            "server-auth"
        },
        resume.label(),
        (rounds as f64) / client_time
    );
    println!(
        "handshakes\t{:?}\t{:?}\t{:?}\tserver\t{}\t{}\t{:.2}\thandshake/s",
        params.version,
        params.key_type,
        params.ciphersuite.suite,
        if clientauth == ClientAuth::Yes {
            "mutual"
        } else {
            "server-auth"
        },
        resume.label(),
        (rounds as f64) / server_time
    );
}

fn do_handshake_step(client: &mut ClientSession, server: &mut ServerSession) -> bool {
    if server.is_handshaking() || client.is_handshaking() {
        transfer(client, server);
        server.process_new_packets().unwrap();
        transfer(server, client);
        client.process_new_packets().unwrap();
        true
    } else {
        false
    }
}

fn do_handshake(client: &mut ClientSession, server: &mut ServerSession) {
    while do_handshake_step(client, server) {}
}

fn bench_bulk(params: &BenchmarkParam, plaintext_size: u64, mtu: Option<usize>) {
    let client_config = Arc::new(make_client_config(params, ClientAuth::No, Resumption::No));
    let server_config = Arc::new(make_server_config(
        params,
        ClientAuth::No,
        Resumption::No,
        mtu,
    ));

    let dns_name = webpki::DNSNameRef::try_from_ascii_str("localhost").unwrap();
    let mut client = ClientSession::new(&client_config, dns_name);
    let mut server = ServerSession::new(&server_config);

    do_handshake(&mut client, &mut server);

    let mut buf = Vec::new();
    buf.resize(plaintext_size as usize, 0u8);

    let total_data = apply_work_multiplier(if plaintext_size < 8192 {
        64 * 1024 * 1024
    } else {
        1024 * 1024 * 1024
    });
    let rounds = total_data / plaintext_size;
    let mut time_send = 0f64;
    let mut time_recv = 0f64;

    for _ in 0..rounds {
        time_send += time(|| {
            server.write_all(&buf).unwrap();
            ()
        });

        time_recv += transfer(&mut server, &mut client);

        time_recv += time(|| client.process_new_packets().unwrap());
        drain(&mut client, buf.len());
    }

    let mtu_str = format!(
        "mtu:{}",
        mtu.map(|v| v.to_string())
            .unwrap_or("default".to_string())
    );
    let total_mbs = ((plaintext_size * rounds) as f64) / (1024. * 1024.);
    println!(
        "bulk\t{:?}\t{:?}\t{}\tsend\t{:.2}\tMB/s",
        params.version,
        params.ciphersuite.suite,
        mtu_str,
        total_mbs / time_send
    );
    println!(
        "bulk\t{:?}\t{:?}\t{}\trecv\t{:.2}\tMB/s",
        params.version,
        params.ciphersuite.suite,
        mtu_str,
        total_mbs / time_recv
    );
}

fn bench_memory(params: &BenchmarkParam, session_count: u64) {
    let client_config = Arc::new(make_client_config(params, ClientAuth::No, Resumption::No));
    let server_config = Arc::new(make_server_config(
        params,
        ClientAuth::No,
        Resumption::No,
        None,
    ));

    // The target here is to end up with session_count post-handshake
    // server and client sessions.
    let session_count = (session_count / 2) as usize;
    let mut servers = Vec::with_capacity(session_count);
    let mut clients = Vec::with_capacity(session_count);

    for _i in 0..session_count {
        servers.push(ServerSession::new(&server_config));
        let dns_name = webpki::DNSNameRef::try_from_ascii_str("localhost").unwrap();
        clients.push(ClientSession::new(&client_config, dns_name));
    }

    for _step in 0..5 {
        for (mut client, mut server) in clients
            .iter_mut()
            .zip(servers.iter_mut())
        {
            do_handshake_step(&mut client, &mut server);
        }
    }

    for client in clients.iter_mut() {
        client.write_all(&[0u8; 1024]).unwrap();
    }

    for (client, server) in clients
        .iter_mut()
        .zip(servers.iter_mut())
    {
        transfer(client, server);
        let mut buf = [0u8; 1024];
        server.read(&mut buf).unwrap();
    }
}

fn lookup_matching_benches(name: &str) -> Vec<&BenchmarkParam> {
    let r: Vec<&BenchmarkParam> = ALL_BENCHMARKS
        .iter()
        .filter(|params| {
            format!("{:?}", params.ciphersuite.suite).to_lowercase() == name.to_lowercase()
        })
        .collect();

    if r.is_empty() {
        panic!("unknown suite {:?}", name);
    }

    r
}

fn selected_tests(mut args: env::Args) {
    let mode = args
        .next()
        .expect("first argument must be mode");

    match mode.as_ref() {
        "bulk" => match args.next() {
            Some(suite) => {
                let len = args
                    .next()
                    .map(|arg| {
                        arg.parse::<u64>()
                            .expect("3rd arg must be plaintext size integer")
                    })
                    .unwrap_or(1048576);
                let mtu = args.next().map(|arg| {
                    arg.parse::<usize>()
                        .expect("4th arg must be mtu integer")
                });
                for param in lookup_matching_benches(&suite).iter() {
                    bench_bulk(param, len, mtu);
                }
            }
            None => {
                panic!("bulk needs ciphersuite argument");
            }
        },

        "handshake" | "handshake-resume" | "handshake-ticket" => match args.next() {
            Some(suite) => {
                let resume = if mode == "handshake" {
                    Resumption::No
                } else if mode == "handshake-resume" {
                    Resumption::SessionID
                } else {
                    Resumption::Tickets
                };

                for param in lookup_matching_benches(&suite).iter() {
                    bench_handshake(param, ClientAuth::No, resume);
                }
            }
            None => {
                panic!("handshake* needs ciphersuite argument");
            }
        },

        "memory" => match args.next() {
            Some(suite) => {
                let count = args
                    .next()
                    .map(|arg| {
                        arg.parse::<u64>()
                            .expect("3rd arg must be session count integer")
                    })
                    .unwrap_or(1000000);
                for param in lookup_matching_benches(&suite).iter() {
                    bench_memory(param, count);
                }
            }
            None => {
                panic!("memory needs ciphersuite argument");
            }
        },

        _ => {
            panic!("unsupported mode {:?}", mode);
        }
    }
}

fn all_tests() {
    for test in ALL_BENCHMARKS.iter() {
        bench_bulk(test, 1024 * 1024, None);
        bench_bulk(test, 1024 * 1024, Some(10000));
        bench_handshake(test, ClientAuth::No, Resumption::No);
        bench_handshake(test, ClientAuth::Yes, Resumption::No);
        bench_handshake(test, ClientAuth::No, Resumption::SessionID);
        bench_handshake(test, ClientAuth::Yes, Resumption::SessionID);
        bench_handshake(test, ClientAuth::No, Resumption::Tickets);
        bench_handshake(test, ClientAuth::Yes, Resumption::Tickets);
    }
}

fn main() {
    let mut args = env::args();
    if args.len() > 1 {
        args.next();
        selected_tests(args);
    } else {
        all_tests();
    }
}
