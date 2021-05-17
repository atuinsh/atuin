#![cfg(feature = "rand")]

extern crate num_bigint;
extern crate num_traits;
extern crate rand;

mod biguint {
    use num_bigint::{BigUint, RandBigInt, RandomBits};
    use num_traits::Zero;
    use rand::distributions::Uniform;
    use rand::thread_rng;
    use rand::{Rng, SeedableRng};

    #[test]
    fn test_rand() {
        let mut rng = thread_rng();
        let n: BigUint = rng.gen_biguint(137);
        assert!(n.bits() <= 137);
        assert!(rng.gen_biguint(0).is_zero());
    }

    #[test]
    fn test_rand_bits() {
        let mut rng = thread_rng();
        let n: BigUint = rng.sample(&RandomBits::new(137));
        assert!(n.bits() <= 137);
        let z: BigUint = rng.sample(&RandomBits::new(0));
        assert!(z.is_zero());
    }

    #[test]
    fn test_rand_range() {
        let mut rng = thread_rng();

        for _ in 0..10 {
            assert_eq!(
                rng.gen_biguint_range(&BigUint::from(236u32), &BigUint::from(237u32)),
                BigUint::from(236u32)
            );
        }

        let l = BigUint::from(403469000u32 + 2352);
        let u = BigUint::from(403469000u32 + 3513);
        for _ in 0..1000 {
            let n: BigUint = rng.gen_biguint_below(&u);
            assert!(n < u);

            let n: BigUint = rng.gen_biguint_range(&l, &u);
            assert!(n >= l);
            assert!(n < u);
        }
    }

    #[test]
    #[should_panic]
    fn test_zero_rand_range() {
        thread_rng().gen_biguint_range(&BigUint::from(54u32), &BigUint::from(54u32));
    }

    #[test]
    #[should_panic]
    fn test_negative_rand_range() {
        let mut rng = thread_rng();
        let l = BigUint::from(2352u32);
        let u = BigUint::from(3513u32);
        // Switching u and l should fail:
        let _n: BigUint = rng.gen_biguint_range(&u, &l);
    }

    #[test]
    fn test_rand_uniform() {
        let mut rng = thread_rng();

        let tiny = Uniform::new(BigUint::from(236u32), BigUint::from(237u32));
        for _ in 0..10 {
            assert_eq!(rng.sample(&tiny), BigUint::from(236u32));
        }

        let l = BigUint::from(403469000u32 + 2352);
        let u = BigUint::from(403469000u32 + 3513);
        let below = Uniform::new(BigUint::zero(), u.clone());
        let range = Uniform::new(l.clone(), u.clone());
        for _ in 0..1000 {
            let n: BigUint = rng.sample(&below);
            assert!(n < u);

            let n: BigUint = rng.sample(&range);
            assert!(n >= l);
            assert!(n < u);
        }
    }

    fn seeded_value_stability<R: SeedableRng + RandBigInt>(expected: &[&str]) {
        let mut seed = <R::Seed>::default();
        for (i, x) in seed.as_mut().iter_mut().enumerate() {
            *x = (i as u8).wrapping_mul(191);
        }
        let mut rng = R::from_seed(seed);
        for (i, &s) in expected.iter().enumerate() {
            let n: BigUint = s.parse().unwrap();
            let r = rng.gen_biguint((1 << i) + i);
            assert_eq!(n, r);
        }
    }

    #[test]
    fn test_chacha_value_stability() {
        const EXPECTED: &[&str] = &[
            "0",
            "0",
            "52",
            "84",
            "23780",
            "86502865016",
            "187057847319509867386",
            "34045731223080904464438757488196244981910",
            "23813754422987836414755953516143692594193066497413249270287126597896871975915808",
            "57401636903146945411652549098818446911814352529449356393690984105383482703074355\
             67088360974672291353736011718191813678720755501317478656550386324355699624671",
        ];
        use rand::prng::ChaChaRng;
        seeded_value_stability::<ChaChaRng>(EXPECTED);
    }

    #[test]
    fn test_isaac_value_stability() {
        const EXPECTED: &[&str] = &[
            "1",
            "4",
            "3",
            "649",
            "89116",
            "7730042024",
            "20773149082453254949",
            "35999009049239918667571895439206839620281",
            "10191757312714088681302309313551624007714035309632506837271600807524767413673006",
            "37805949268912387809989378008822038725134260145886913321084097194957861133272558\
             43458183365174899239251448892645546322463253898288141861183340823194379722556",
        ];
        use rand::prng::IsaacRng;
        seeded_value_stability::<IsaacRng>(EXPECTED);
    }

    #[test]
    fn test_xorshift_value_stability() {
        const EXPECTED: &[&str] = &[
            "1",
            "0",
            "37",
            "395",
            "181116",
            "122718231117",
            "1068467172329355695001",
            "28246925743544411614293300167064395633287",
            "12750053187017853048648861493745244146555950255549630854523304068318587267293038",
            "53041498719137109355568081064978196049094604705283682101683207799515709404788873\
             53417136457745727045473194367732849819278740266658219147356315674940229288531",
        ];
        use rand::prng::XorShiftRng;
        seeded_value_stability::<XorShiftRng>(EXPECTED);
    }
}

mod bigint {
    use num_bigint::{BigInt, RandBigInt, RandomBits};
    use num_traits::Zero;
    use rand::distributions::Uniform;
    use rand::thread_rng;
    use rand::{Rng, SeedableRng};

    #[test]
    fn test_rand() {
        let mut rng = thread_rng();
        let n: BigInt = rng.gen_bigint(137);
        assert!(n.bits() <= 137);
        assert!(rng.gen_bigint(0).is_zero());
    }

    #[test]
    fn test_rand_bits() {
        let mut rng = thread_rng();
        let n: BigInt = rng.sample(&RandomBits::new(137));
        assert!(n.bits() <= 137);
        let z: BigInt = rng.sample(&RandomBits::new(0));
        assert!(z.is_zero());
    }

    #[test]
    fn test_rand_range() {
        let mut rng = thread_rng();

        for _ in 0..10 {
            assert_eq!(
                rng.gen_bigint_range(&BigInt::from(236), &BigInt::from(237)),
                BigInt::from(236)
            );
        }

        fn check(l: BigInt, u: BigInt) {
            let mut rng = thread_rng();
            for _ in 0..1000 {
                let n: BigInt = rng.gen_bigint_range(&l, &u);
                assert!(n >= l);
                assert!(n < u);
            }
        }
        let l: BigInt = BigInt::from(403469000 + 2352);
        let u: BigInt = BigInt::from(403469000 + 3513);
        check(l.clone(), u.clone());
        check(-l.clone(), u.clone());
        check(-u.clone(), -l.clone());
    }

    #[test]
    #[should_panic]
    fn test_zero_rand_range() {
        thread_rng().gen_bigint_range(&BigInt::from(54), &BigInt::from(54));
    }

    #[test]
    #[should_panic]
    fn test_negative_rand_range() {
        let mut rng = thread_rng();
        let l = BigInt::from(2352);
        let u = BigInt::from(3513);
        // Switching u and l should fail:
        let _n: BigInt = rng.gen_bigint_range(&u, &l);
    }

    #[test]
    fn test_rand_uniform() {
        let mut rng = thread_rng();

        let tiny = Uniform::new(BigInt::from(236u32), BigInt::from(237u32));
        for _ in 0..10 {
            assert_eq!(rng.sample(&tiny), BigInt::from(236u32));
        }

        fn check(l: BigInt, u: BigInt) {
            let mut rng = thread_rng();
            let range = Uniform::new(l.clone(), u.clone());
            for _ in 0..1000 {
                let n: BigInt = rng.sample(&range);
                assert!(n >= l);
                assert!(n < u);
            }
        }
        let l: BigInt = BigInt::from(403469000 + 2352);
        let u: BigInt = BigInt::from(403469000 + 3513);
        check(l.clone(), u.clone());
        check(-l.clone(), u.clone());
        check(-u.clone(), -l.clone());
    }

    fn seeded_value_stability<R: SeedableRng + RandBigInt>(expected: &[&str]) {
        let mut seed = <R::Seed>::default();
        for (i, x) in seed.as_mut().iter_mut().enumerate() {
            *x = (i as u8).wrapping_mul(191);
        }
        let mut rng = R::from_seed(seed);
        for (i, &s) in expected.iter().enumerate() {
            let n: BigInt = s.parse().unwrap();
            let r = rng.gen_bigint((1 << i) + i);
            assert_eq!(n, r);
        }
    }

    #[test]
    fn test_chacha_value_stability() {
        const EXPECTED: &[&str] = &[
            "0",
            "-6",
            "-1",
            "1321",
            "-147247",
            "8486373526",
            "-272736656290199720696",
            "2731152629387534140535423510744221288522",
            "-28820024790651190394679732038637785320661450462089347915910979466834461433196572",
            "501454570554170484799723603981439288209930393334472085317977614690773821680884844\
             8530978478667288338327570972869032358120588620346111979053742269317702532328",
        ];
        use rand::prng::ChaChaRng;
        seeded_value_stability::<ChaChaRng>(EXPECTED);
    }

    #[test]
    fn test_isaac_value_stability() {
        const EXPECTED: &[&str] = &[
            "1",
            "0",
            "5",
            "113",
            "-132240",
            "-36348760761",
            "-365690596708430705434",
            "-14090753008246284277803606722552430292432",
            "-26313941628626248579319341019368550803676255307056857978955881718727601479436059",
            "-14563174552421101848999036239003801073335703811160945137332228646111920972691151\
             88341090358094331641182310792892459091016794928947242043358702692294695845817",
        ];
        use rand::prng::IsaacRng;
        seeded_value_stability::<IsaacRng>(EXPECTED);
    }

    #[test]
    fn test_xorshift_value_stability() {
        const EXPECTED: &[&str] = &[
            "-1",
            "-4",
            "11",
            "-1802",
            "966495",
            "-62592045703",
            "-602281783447192077116",
            "-34335811410223060575607987996861632509125",
            "29156580925282215857325937227200350542000244609280383263289720243118706105351199",
            "49920038676141573457451407325930326489996232208489690499754573826911037849083623\
             24546142615325187412887314466195222441945661833644117700809693098722026764846",
        ];
        use rand::prng::XorShiftRng;
        seeded_value_stability::<XorShiftRng>(EXPECTED);
    }
}
