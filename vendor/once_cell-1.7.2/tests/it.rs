mod unsync {
    use core::{
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering::SeqCst},
    };

    use once_cell::unsync::{Lazy, OnceCell};

    #[test]
    fn once_cell() {
        let c = OnceCell::new();
        assert!(c.get().is_none());
        c.get_or_init(|| 92);
        assert_eq!(c.get(), Some(&92));

        c.get_or_init(|| panic!("Kabom!"));
        assert_eq!(c.get(), Some(&92));
    }

    #[test]
    fn once_cell_get_mut() {
        let mut c = OnceCell::new();
        assert!(c.get_mut().is_none());
        c.set(90).unwrap();
        *c.get_mut().unwrap() += 2;
        assert_eq!(c.get_mut(), Some(&mut 92));
    }

    #[test]
    fn once_cell_drop() {
        static DROP_CNT: AtomicUsize = AtomicUsize::new(0);
        struct Dropper;
        impl Drop for Dropper {
            fn drop(&mut self) {
                DROP_CNT.fetch_add(1, SeqCst);
            }
        }

        let x = OnceCell::new();
        x.get_or_init(|| Dropper);
        assert_eq!(DROP_CNT.load(SeqCst), 0);
        drop(x);
        assert_eq!(DROP_CNT.load(SeqCst), 1);
    }

    #[test]
    fn unsync_once_cell_drop_empty() {
        let x = OnceCell::<String>::new();
        drop(x);
    }

    #[test]
    fn clone() {
        let s = OnceCell::new();
        let c = s.clone();
        assert!(c.get().is_none());

        s.set("hello".to_string()).unwrap();
        let c = s.clone();
        assert_eq!(c.get().map(String::as_str), Some("hello"));
    }

    #[test]
    fn from_impl() {
        assert_eq!(OnceCell::from("value").get(), Some(&"value"));
        assert_ne!(OnceCell::from("foo").get(), Some(&"bar"));
    }

    #[test]
    fn partialeq_impl() {
        assert!(OnceCell::from("value") == OnceCell::from("value"));
        assert!(OnceCell::from("foo") != OnceCell::from("bar"));

        assert!(OnceCell::<String>::new() == OnceCell::new());
        assert!(OnceCell::<String>::new() != OnceCell::from("value".to_owned()));
    }

    #[test]
    fn into_inner() {
        let cell: OnceCell<String> = OnceCell::new();
        assert_eq!(cell.into_inner(), None);
        let cell = OnceCell::new();
        cell.set("hello".to_string()).unwrap();
        assert_eq!(cell.into_inner(), Some("hello".to_string()));
    }

    #[test]
    fn debug_impl() {
        let cell = OnceCell::new();
        assert_eq!(format!("{:?}", cell), "OnceCell(Uninit)");
        cell.set("hello".to_string()).unwrap();
        assert_eq!(format!("{:?}", cell), "OnceCell(\"hello\")");
    }

    #[test]
    fn lazy_new() {
        let called = Cell::new(0);
        let x = Lazy::new(|| {
            called.set(called.get() + 1);
            92
        });

        assert_eq!(called.get(), 0);

        let y = *x - 30;
        assert_eq!(y, 62);
        assert_eq!(called.get(), 1);

        let y = *x - 30;
        assert_eq!(y, 62);
        assert_eq!(called.get(), 1);
    }

    #[test]
    fn lazy_deref_mut() {
        let called = Cell::new(0);
        let mut x = Lazy::new(|| {
            called.set(called.get() + 1);
            92
        });

        assert_eq!(called.get(), 0);

        let y = *x - 30;
        assert_eq!(y, 62);
        assert_eq!(called.get(), 1);

        *x /= 2;
        assert_eq!(*x, 46);
        assert_eq!(called.get(), 1);
    }

    #[test]
    fn lazy_default() {
        static CALLED: AtomicUsize = AtomicUsize::new(0);

        struct Foo(u8);
        impl Default for Foo {
            fn default() -> Self {
                CALLED.fetch_add(1, SeqCst);
                Foo(42)
            }
        }

        let lazy: Lazy<std::sync::Mutex<Foo>> = <_>::default();

        assert_eq!(CALLED.load(SeqCst), 0);

        assert_eq!(lazy.lock().unwrap().0, 42);
        assert_eq!(CALLED.load(SeqCst), 1);

        lazy.lock().unwrap().0 = 21;

        assert_eq!(lazy.lock().unwrap().0, 21);
        assert_eq!(CALLED.load(SeqCst), 1);
    }

    #[test]
    fn lazy_into_value() {
        let l: Lazy<i32, _> = Lazy::new(|| panic!());
        assert!(matches!(Lazy::into_value(l), Err(_)));
        let l = Lazy::new(|| -> i32 { 92 });
        Lazy::force(&l);
        assert!(matches!(Lazy::into_value(l), Ok(92)));
    }

    #[test]
    #[cfg(feature = "std")]
    fn lazy_poisoning() {
        let x: Lazy<String> = Lazy::new(|| panic!("kaboom"));
        for _ in 0..2 {
            let res = std::panic::catch_unwind(|| x.len());
            assert!(res.is_err());
        }
    }

    #[test]
    fn aliasing_in_get() {
        let x = OnceCell::new();
        x.set(42).unwrap();
        let at_x = x.get().unwrap(); // --- (shared) borrow of inner `Option<T>` --+
        let _ = x.set(27); // <-- temporary (unique) borrow of inner `Option<T>`   |
        println!("{}", at_x); // <------- up until here ---------------------------+
    }

    #[test]
    #[should_panic(expected = "reentrant init")]
    fn reentrant_init() {
        let x: OnceCell<Box<i32>> = OnceCell::new();
        let dangling_ref: Cell<Option<&i32>> = Cell::new(None);
        x.get_or_init(|| {
            let r = x.get_or_init(|| Box::new(92));
            dangling_ref.set(Some(r));
            Box::new(62)
        });
        eprintln!("use after free: {:?}", dangling_ref.get().unwrap());
    }

    #[test]
    // https://github.com/rust-lang/rust/issues/34761#issuecomment-256320669
    fn arrrrrrrrrrrrrrrrrrrrrr() {
        let cell = OnceCell::new();
        {
            let s = String::new();
            cell.set(&s).unwrap();
        }
    }
}

#[cfg(feature = "std")]
mod sync {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    use crossbeam_utils::thread::scope;

    use once_cell::sync::{Lazy, OnceCell};

    #[test]
    fn once_cell() {
        let c = OnceCell::new();
        assert!(c.get().is_none());
        scope(|s| {
            s.spawn(|_| {
                c.get_or_init(|| 92);
                assert_eq!(c.get(), Some(&92));
            });
        })
        .unwrap();
        c.get_or_init(|| panic!("Kabom!"));
        assert_eq!(c.get(), Some(&92));
    }

    #[test]
    fn once_cell_get_mut() {
        let mut c = OnceCell::new();
        assert!(c.get_mut().is_none());
        c.set(90).unwrap();
        *c.get_mut().unwrap() += 2;
        assert_eq!(c.get_mut(), Some(&mut 92));
    }

    #[test]
    fn once_cell_get_unchecked() {
        let c = OnceCell::new();
        c.set(92).unwrap();
        unsafe {
            assert_eq!(c.get_unchecked(), &92);
        }
    }

    #[test]
    fn once_cell_drop() {
        static DROP_CNT: AtomicUsize = AtomicUsize::new(0);
        struct Dropper;
        impl Drop for Dropper {
            fn drop(&mut self) {
                DROP_CNT.fetch_add(1, SeqCst);
            }
        }

        let x = OnceCell::new();
        scope(|s| {
            s.spawn(|_| {
                x.get_or_init(|| Dropper);
                assert_eq!(DROP_CNT.load(SeqCst), 0);
                drop(x);
            });
        })
        .unwrap();
        assert_eq!(DROP_CNT.load(SeqCst), 1);
    }

    #[test]
    fn once_cell_drop_empty() {
        let x = OnceCell::<String>::new();
        drop(x);
    }

    #[test]
    fn clone() {
        let s = OnceCell::new();
        let c = s.clone();
        assert!(c.get().is_none());

        s.set("hello".to_string()).unwrap();
        let c = s.clone();
        assert_eq!(c.get().map(String::as_str), Some("hello"));
    }

    #[test]
    fn get_or_try_init() {
        let cell: OnceCell<String> = OnceCell::new();
        assert!(cell.get().is_none());

        let res =
            std::panic::catch_unwind(|| cell.get_or_try_init(|| -> Result<_, ()> { panic!() }));
        assert!(res.is_err());
        assert!(cell.get().is_none());

        assert_eq!(cell.get_or_try_init(|| Err(())), Err(()));

        assert_eq!(
            cell.get_or_try_init(|| Ok::<_, ()>("hello".to_string())),
            Ok(&"hello".to_string())
        );
        assert_eq!(cell.get(), Some(&"hello".to_string()));
    }

    #[test]
    fn from_impl() {
        assert_eq!(OnceCell::from("value").get(), Some(&"value"));
        assert_ne!(OnceCell::from("foo").get(), Some(&"bar"));
    }

    #[test]
    fn partialeq_impl() {
        assert!(OnceCell::from("value") == OnceCell::from("value"));
        assert!(OnceCell::from("foo") != OnceCell::from("bar"));

        assert!(OnceCell::<String>::new() == OnceCell::new());
        assert!(OnceCell::<String>::new() != OnceCell::from("value".to_owned()));
    }

    #[test]
    fn into_inner() {
        let cell: OnceCell<String> = OnceCell::new();
        assert_eq!(cell.into_inner(), None);
        let cell = OnceCell::new();
        cell.set("hello".to_string()).unwrap();
        assert_eq!(cell.into_inner(), Some("hello".to_string()));
    }

    #[test]
    fn debug_impl() {
        let cell = OnceCell::new();
        assert_eq!(format!("{:#?}", cell), "OnceCell(Uninit)");
        cell.set(vec!["hello", "world"]).unwrap();
        assert_eq!(
            format!("{:#?}", cell),
            r#"OnceCell(
    [
        "hello",
        "world",
    ],
)"#
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // miri doesn't support processes
    fn reentrant_init() {
        let examples_dir = {
            let mut exe = std::env::current_exe().unwrap();
            exe.pop();
            exe.pop();
            exe.push("examples");
            exe
        };
        let bin = examples_dir
            .join("reentrant_init_deadlocks")
            .with_extension(std::env::consts::EXE_EXTENSION);
        let mut guard = Guard { child: std::process::Command::new(bin).spawn().unwrap() };
        std::thread::sleep(std::time::Duration::from_secs(2));
        let status = guard.child.try_wait().unwrap();
        assert!(status.is_none());

        struct Guard {
            child: std::process::Child,
        }

        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = self.child.kill();
            }
        }
    }

    #[test]
    fn lazy_new() {
        let called = AtomicUsize::new(0);
        let x = Lazy::new(|| {
            called.fetch_add(1, SeqCst);
            92
        });

        assert_eq!(called.load(SeqCst), 0);

        scope(|s| {
            s.spawn(|_| {
                let y = *x - 30;
                assert_eq!(y, 62);
                assert_eq!(called.load(SeqCst), 1);
            });
        })
        .unwrap();

        let y = *x - 30;
        assert_eq!(y, 62);
        assert_eq!(called.load(SeqCst), 1);
    }

    #[test]
    fn lazy_deref_mut() {
        let called = AtomicUsize::new(0);
        let mut x = Lazy::new(|| {
            called.fetch_add(1, SeqCst);
            92
        });

        assert_eq!(called.load(SeqCst), 0);

        let y = *x - 30;
        assert_eq!(y, 62);
        assert_eq!(called.load(SeqCst), 1);

        *x /= 2;
        assert_eq!(*x, 46);
        assert_eq!(called.load(SeqCst), 1);
    }

    #[test]
    fn lazy_default() {
        static CALLED: AtomicUsize = AtomicUsize::new(0);

        struct Foo(u8);
        impl Default for Foo {
            fn default() -> Self {
                CALLED.fetch_add(1, SeqCst);
                Foo(42)
            }
        }

        let lazy: Lazy<std::sync::Mutex<Foo>> = <_>::default();

        assert_eq!(CALLED.load(SeqCst), 0);

        assert_eq!(lazy.lock().unwrap().0, 42);
        assert_eq!(CALLED.load(SeqCst), 1);

        lazy.lock().unwrap().0 = 21;

        assert_eq!(lazy.lock().unwrap().0, 21);
        assert_eq!(CALLED.load(SeqCst), 1);
    }

    #[test]
    fn static_lazy() {
        static XS: Lazy<Vec<i32>> = Lazy::new(|| {
            let mut xs = Vec::new();
            xs.push(1);
            xs.push(2);
            xs.push(3);
            xs
        });
        scope(|s| {
            s.spawn(|_| {
                assert_eq!(&*XS, &vec![1, 2, 3]);
            });
        })
        .unwrap();
        assert_eq!(&*XS, &vec![1, 2, 3]);
    }

    #[test]
    fn static_lazy_via_fn() {
        fn xs() -> &'static Vec<i32> {
            static XS: OnceCell<Vec<i32>> = OnceCell::new();
            XS.get_or_init(|| {
                let mut xs = Vec::new();
                xs.push(1);
                xs.push(2);
                xs.push(3);
                xs
            })
        }
        assert_eq!(xs(), &vec![1, 2, 3]);
    }

    #[test]
    fn lazy_into_value() {
        let l: Lazy<i32, _> = Lazy::new(|| panic!());
        assert!(matches!(Lazy::into_value(l), Err(_)));
        let l = Lazy::new(|| -> i32 { 92 });
        Lazy::force(&l);
        assert!(matches!(Lazy::into_value(l), Ok(92)));
    }

    #[test]
    fn lazy_poisoning() {
        let x: Lazy<String> = Lazy::new(|| panic!("kaboom"));
        for _ in 0..2 {
            let res = std::panic::catch_unwind(|| x.len());
            assert!(res.is_err());
        }
    }

    #[test]
    fn once_cell_is_sync_send() {
        fn assert_traits<T: Send + Sync>() {}
        assert_traits::<OnceCell<String>>();
        assert_traits::<Lazy<String>>();
    }

    #[test]
    fn eval_once_macro() {
        macro_rules! eval_once {
            (|| -> $ty:ty {
                $($body:tt)*
            }) => {{
                static ONCE_CELL: OnceCell<$ty> = OnceCell::new();
                fn init() -> $ty {
                    $($body)*
                }
                ONCE_CELL.get_or_init(init)
            }};
        }

        let fib: &'static Vec<i32> = eval_once! {
            || -> Vec<i32> {
                let mut res = vec![1, 1];
                for i in 0..10 {
                    let next = res[i] + res[i + 1];
                    res.push(next);
                }
                res
            }
        };
        assert_eq!(fib[5], 8)
    }

    #[test]
    #[cfg_attr(miri, ignore)] // FIXME: deadlocks, likely caused by https://github.com/rust-lang/miri/issues/1388
    fn once_cell_does_not_leak_partially_constructed_boxes() {
        let n_tries = 100;
        let n_readers = 10;
        let n_writers = 3;
        const MSG: &str = "Hello, World";

        for _ in 0..n_tries {
            let cell: OnceCell<String> = OnceCell::new();
            scope(|scope| {
                for _ in 0..n_readers {
                    scope.spawn(|_| loop {
                        if let Some(msg) = cell.get() {
                            assert_eq!(msg, MSG);
                            break;
                        }
                    });
                }
                for _ in 0..n_writers {
                    let _ = scope.spawn(|_| cell.set(MSG.to_owned()));
                }
            })
            .unwrap()
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // miri doesn't support Barrier
    fn get_does_not_block() {
        use std::sync::Barrier;

        let cell = OnceCell::new();
        let barrier = Barrier::new(2);
        scope(|scope| {
            scope.spawn(|_| {
                cell.get_or_init(|| {
                    barrier.wait();
                    barrier.wait();
                    "hello".to_string()
                });
            });
            barrier.wait();
            assert_eq!(cell.get(), None);
            barrier.wait();
        })
        .unwrap();
        assert_eq!(cell.get(), Some(&"hello".to_string()));
    }

    #[test]
    // https://github.com/rust-lang/rust/issues/34761#issuecomment-256320669
    fn arrrrrrrrrrrrrrrrrrrrrr() {
        let cell = OnceCell::new();
        {
            let s = String::new();
            cell.set(&s).unwrap();
        }
    }
}

#[cfg(feature = "race")]
mod race {
    use std::{
        num::NonZeroUsize,
        sync::{
            atomic::{AtomicUsize, Ordering::SeqCst},
            Barrier,
        },
    };

    use crossbeam_utils::thread::scope;

    use once_cell::race::{OnceBool, OnceNonZeroUsize};

    #[test]
    fn once_non_zero_usize_smoke_test() {
        let cnt = AtomicUsize::new(0);
        let cell = OnceNonZeroUsize::new();
        let val = NonZeroUsize::new(92).unwrap();
        scope(|s| {
            s.spawn(|_| {
                assert_eq!(
                    cell.get_or_init(|| {
                        cnt.fetch_add(1, SeqCst);
                        val
                    }),
                    val
                );
                assert_eq!(cnt.load(SeqCst), 1);

                assert_eq!(
                    cell.get_or_init(|| {
                        cnt.fetch_add(1, SeqCst);
                        val
                    }),
                    val
                );
                assert_eq!(cnt.load(SeqCst), 1);
            });
        })
        .unwrap();
        assert_eq!(cell.get(), Some(val));
        assert_eq!(cnt.load(SeqCst), 1);
    }

    #[test]
    fn once_non_zero_usize_set() {
        let val1 = NonZeroUsize::new(92).unwrap();
        let val2 = NonZeroUsize::new(62).unwrap();

        let cell = OnceNonZeroUsize::new();

        assert!(cell.set(val1).is_ok());
        assert_eq!(cell.get(), Some(val1));

        assert!(cell.set(val2).is_err());
        assert_eq!(cell.get(), Some(val1));
    }

    #[test]
    fn once_non_zero_usize_first_wins() {
        let val1 = NonZeroUsize::new(92).unwrap();
        let val2 = NonZeroUsize::new(62).unwrap();

        let cell = OnceNonZeroUsize::new();

        let b1 = Barrier::new(2);
        let b2 = Barrier::new(2);
        let b3 = Barrier::new(2);
        scope(|s| {
            s.spawn(|_| {
                let r1 = cell.get_or_init(|| {
                    b1.wait();
                    b2.wait();
                    val1
                });
                assert_eq!(r1, val1);
                b3.wait();
            });
            b1.wait();
            s.spawn(|_| {
                let r2 = cell.get_or_init(|| {
                    b2.wait();
                    b3.wait();
                    val2
                });
                assert_eq!(r2, val1);
            });
        })
        .unwrap();

        assert_eq!(cell.get(), Some(val1));
    }

    #[test]
    fn once_bool_smoke_test() {
        let cnt = AtomicUsize::new(0);
        let cell = OnceBool::new();
        scope(|s| {
            s.spawn(|_| {
                assert_eq!(
                    cell.get_or_init(|| {
                        cnt.fetch_add(1, SeqCst);
                        false
                    }),
                    false
                );
                assert_eq!(cnt.load(SeqCst), 1);

                assert_eq!(
                    cell.get_or_init(|| {
                        cnt.fetch_add(1, SeqCst);
                        false
                    }),
                    false
                );
                assert_eq!(cnt.load(SeqCst), 1);
            });
        })
        .unwrap();
        assert_eq!(cell.get(), Some(false));
        assert_eq!(cnt.load(SeqCst), 1);
    }

    #[test]
    fn once_bool_set() {
        let cell = OnceBool::new();

        assert!(cell.set(false).is_ok());
        assert_eq!(cell.get(), Some(false));

        assert!(cell.set(true).is_err());
        assert_eq!(cell.get(), Some(false));
    }
}

#[cfg(all(feature = "race", feature = "alloc"))]
mod race_once_box {
    use std::sync::{
        atomic::{AtomicUsize, Ordering::SeqCst},
        Arc, Barrier,
    };

    use crossbeam_utils::thread::scope;
    use once_cell::race::OnceBox;

    #[derive(Default)]
    struct Heap {
        total: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct Pebble<T> {
        val: T,
        total: Arc<AtomicUsize>,
    }

    impl<T> Drop for Pebble<T> {
        fn drop(&mut self) {
            self.total.fetch_sub(1, SeqCst);
        }
    }

    impl Heap {
        fn total(&self) -> usize {
            self.total.load(SeqCst)
        }
        fn new_pebble<T>(&self, val: T) -> Pebble<T> {
            self.total.fetch_add(1, SeqCst);
            Pebble { val, total: Arc::clone(&self.total) }
        }
    }

    #[test]
    fn once_box_smoke_test() {
        let heap = Heap::default();
        let global_cnt = AtomicUsize::new(0);
        let cell = OnceBox::new();
        let b = Barrier::new(128);
        scope(|s| {
            for _ in 0..128 {
                s.spawn(|_| {
                    let local_cnt = AtomicUsize::new(0);
                    cell.get_or_init(|| {
                        global_cnt.fetch_add(1, SeqCst);
                        local_cnt.fetch_add(1, SeqCst);
                        b.wait();
                        Box::new(heap.new_pebble(()))
                    });
                    assert_eq!(local_cnt.load(SeqCst), 1);

                    cell.get_or_init(|| {
                        global_cnt.fetch_add(1, SeqCst);
                        local_cnt.fetch_add(1, SeqCst);
                        Box::new(heap.new_pebble(()))
                    });
                    assert_eq!(local_cnt.load(SeqCst), 1);
                });
            }
        })
        .unwrap();
        assert!(cell.get().is_some());
        assert!(global_cnt.load(SeqCst) > 10);

        assert_eq!(heap.total(), 1);
        drop(cell);
        assert_eq!(heap.total(), 0);
    }

    #[test]
    fn once_box_set() {
        let heap = Heap::default();
        let cell = OnceBox::new();
        assert!(cell.get().is_none());

        assert!(cell.set(Box::new(heap.new_pebble("hello"))).is_ok());
        assert_eq!(cell.get().unwrap().val, "hello");
        assert_eq!(heap.total(), 1);

        assert!(cell.set(Box::new(heap.new_pebble("world"))).is_err());
        assert_eq!(cell.get().unwrap().val, "hello");
        assert_eq!(heap.total(), 1);

        drop(cell);
        assert_eq!(heap.total(), 0);
    }

    #[test]
    fn once_box_first_wins() {
        let cell = OnceBox::new();
        let val1 = 92;
        let val2 = 62;

        let b1 = Barrier::new(2);
        let b2 = Barrier::new(2);
        let b3 = Barrier::new(2);
        scope(|s| {
            s.spawn(|_| {
                let r1 = cell.get_or_init(|| {
                    b1.wait();
                    b2.wait();
                    Box::new(val1)
                });
                assert_eq!(*r1, val1);
                b3.wait();
            });
            b1.wait();
            s.spawn(|_| {
                let r2 = cell.get_or_init(|| {
                    b2.wait();
                    b3.wait();
                    Box::new(val2)
                });
                assert_eq!(*r2, val1);
            });
        })
        .unwrap();

        assert_eq!(cell.get(), Some(&val1));
    }

    #[test]
    fn once_box_reentrant() {
        let cell = OnceBox::new();
        let res = cell.get_or_init(|| {
            cell.get_or_init(|| Box::new("hello".to_string()));
            Box::new("world".to_string())
        });
        assert_eq!(res, "hello");
    }

    #[test]
    fn once_box_default() {
        struct Foo;

        let cell: OnceBox<Foo> = Default::default();
        assert!(cell.get().is_none());
    }
}
