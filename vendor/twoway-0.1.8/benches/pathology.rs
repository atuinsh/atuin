#![feature(test)]
#![feature(
    pattern, )]

#![allow(unused_imports)]
extern crate test;
#[cfg(feature = "jetscii")]
extern crate jetscii;
extern crate itertools;
extern crate odds;
#[cfg(feature = "benchmarks")]
extern crate galil_seiferas;
extern crate unchecked_index;

extern crate twoway;

#[cfg(unused)]
macro_rules! regex {
    ($e:expr) => (::regex::Regex::new($e).unwrap());
}

pub use twoway::{Str};

use std::str::pattern::{Pattern, Searcher, ReverseSearcher};
use test::{Bencher, black_box};

use twoway::find_str as tw_find;
use twoway::rfind_str as tw_rfind;

pub fn is_prefix(text: &str, pattern: &str) -> bool {
    Str(pattern).is_prefix_of(text)
}

pub fn memmem(text: &str, pattern: &str) -> bool {
    #[allow(improper_ctypes)]
    extern { fn memmem(s1: *const u8, m: usize, s2: *const u8, pattern: usize) -> *const u8; }
    unsafe {
        memmem(text.as_ptr(),
               text.len(),
               pattern.as_ptr(),
               pattern.len()) != 0 as *mut u8
    }

}

macro_rules! get {
    ($slice:expr, $index:expr) => {
        unsafe { ::unchecked_index::get_unchecked(&$slice, $index) }
    }
}

fn brute_force<T: Eq>(text: &[T], pattern: &[T]) -> Option<usize> {
    let n = text.len();
    let m = pattern.len();
    if n < m {
        return None;
    }
    'outer: for i in 0..n - m + 1 {

        /* to use memcmp:
         * it's a tradeoff; memcmp is faster with more pathological-y inputs!
         * for relistic inputs where we quickly find a mismatch at most
         * postions, it's faster using just single element get.
        if get!(text, i .. i + m) == pattern {
            return Some(i);
        }
        */

        for j in 0..m {
            if get!(text, i + j) != get!(pattern, j) {
                continue 'outer;
            }
        }
        return Some(i);
    }
    None
}


macro_rules! bench_contains_vs_tw {
    ($name: ident, $hay: expr, $n: expr) => {
        pub mod $name {
            use super::{test, tw_find, tw_rfind,
                LONG,
                LONG_CY,
            };
            use itertools::Itertools;
            use twoway::TwoWaySearcher;
            use test::{Bencher, black_box};
            #[cfg(feature = "jetscii")]
            use jetscii::Substring;
            use odds::string::StrExt;

            #[bench]
            pub fn find(b: &mut Bencher) {
                let haystack = black_box($hay);
                let needle = black_box($n);
                b.iter(|| {
                    test::black_box(haystack.find(&needle));
                });
                b.bytes = haystack.len() as u64;
            }

            #[bench]
            pub fn rfind(b: &mut Bencher) {
                let haystack = black_box($hay);
                let needle = black_box($n);
                b.iter(|| {
                    test::black_box(haystack.rfind(&needle));
                });
                b.bytes = haystack.len() as u64;
            }

            /*
            #[bench]
            pub fn regex_find(b: &mut Bencher) {
                let haystack = black_box($hay);
                let needle = black_box($n);
                let reg = regex!(&needle);
                b.iter(|| {
                    reg.find(&haystack)
                });
                b.bytes = haystack.len() as u64;
            }
            */

            #[cfg(feature = "jetscii")]
            #[bench]
            pub fn jetscii_find(b: &mut Bencher) {
                let haystack = black_box($hay);
                let needle = black_box($n);
                b.iter(|| {
                    haystack.find(Substring::new(&needle))
                });
                b.bytes = haystack.len() as u64;
            }

            /*
            #[bench]
            pub fn str_is_prefix(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    test::black_box(needle.is_prefix_of(haystack));
                });
                b.bytes = needle.len() as u64;
            }
            */

            /*
            #[bench]
            pub fn str_first_reject(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    test::black_box(needle.into_searcher(haystack).next_reject())
                });
            }
            */

            #[cfg(feature = "pcmp")]
            #[bench]
            pub fn pcmp_find(b: &mut Bencher) {
                let haystack = black_box($hay);
                let needle = black_box($n);
                b.iter(|| {
                    test::black_box(::twoway::pcmp::find(haystack.as_bytes(), needle.as_bytes()));
                });
                b.bytes = haystack.len() as u64;
            }

            #[bench]
            pub fn bmh_find(b: &mut Bencher) {
                let haystack = black_box($hay);
                let needle = black_box($n);
                b.iter(|| {
                    test::black_box(::twoway::bmh::find(haystack.as_bytes(), needle.as_bytes()));
                });
                b.bytes = haystack.len() as u64;
            }

            #[bench]
            pub fn memmem(b: &mut Bencher) {
                let haystack = black_box($hay);
                let needle = black_box($n);
                b.iter(|| {
                    test::black_box(::memmem(&haystack, &needle));
                });
                b.bytes = haystack.len() as u64;
            }

            #[bench]
            pub fn twoway_find(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    test::black_box(tw_find(haystack, needle));
                });
                b.bytes = haystack.len() as u64;
            }


            #[cfg(feature = "benchmarks")]
            #[bench]
            pub fn gs_find(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    ::galil_seiferas::gs_find(haystack.as_bytes(), needle.as_bytes())
                });
                b.bytes = haystack.len() as u64;
            }

            #[bench]
            pub fn brute_force(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    ::brute_force(haystack.as_bytes(), needle.as_bytes())
                });
                b.bytes = haystack.len() as u64;
            }

            #[bench]
            pub fn twoway_rfind(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    test::black_box(tw_rfind(haystack, needle));
                });
                b.bytes = haystack.len() as u64;
            }

            /*
            #[bench]
            pub fn tw_is_prefix(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    test::black_box(is_prefix(haystack, needle));
                });
                b.bytes = needle.len() as u64;
            }
            */

            #[bench]
            pub fn twoway_new(b: &mut Bencher) {
                let needle = black_box($n);
                b.iter(|| {
                    let needle = needle.as_bytes();
                    let t = TwoWaySearcher::new(needle, 1);
                    t
                });
                b.bytes = needle.len() as u64;
            }

            /*
            #[bench]
            pub fn pcmp_is_prefix(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    let l = ::std::cmp::min(needle.len(), haystack.len());
                    l == ::twoway::pcmp::shared_prefix(haystack.as_bytes(), needle.as_bytes())
                });
                b.bytes = needle.len() as u64;
            }
            */

            /*
            #[bench]
            pub fn tw_first_reject(b: &mut Bencher) {
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(&needle);
                    let haystack = test::black_box(&haystack);
                    test::black_box(Str(needle).into_searcher(haystack).next_reject())
                });
            }
            */

            /*
            #[bench]
            pub fn tw_paper(b: &mut Bencher) {
                use twoway::tw::{find_first, Str};
                let haystack = $hay;
                let needle = $n;
                b.iter(|| {
                    let needle = test::black_box(Str(needle.as_bytes()));
                    let haystack = test::black_box(Str(haystack.as_bytes()));
                    test::black_box(find_first(haystack, needle));
                });
                b.bytes = haystack.len() as u64;
            }
            */
        }
    }
}


static LONG: &'static str = "\
Lorem ipsum dolor sit amet, consectetur adipiscing elit. Suspendisse quis lorem sit amet dolor \
ultricies condimentum. Praesent iaculis purus elit, ac malesuada quam malesuada in. Duis sed orci \
eros. Suspendisse sit amet magna mollis, mollis nunc luctus, imperdiet mi. Integer fringilla non \
sem ut lacinia. Fusce varius tortor a risus porttitor hendrerit. Morbi mauris dui, ultricies nec \
tempus vel, gravida nec quam.

In est dui, tincidunt sed tempus interdum, adipiscing laoreet ante. Etiam tempor, tellus quis \
sagittis interdum, nulla purus mattis sem, quis auctor erat odio ac tellus. In nec nunc sit amet \
diam volutpat molestie at sed ipsum. Vestibulum laoreet consequat vulputate. Integer accumsan \
lorem ac dignissim placerat. Suspendisse convallis faucibus lorem. Aliquam erat volutpat. In vel \
eleifend felis. Sed suscipit nulla lorem, sed mollis est sollicitudin et. Nam fermentum egestas \
interdum. Curabitur ut nisi justo.

Sed sollicitudin ipsum tellus, ut condimentum leo eleifend nec. Cras ut velit ante. Phasellus nec \
mollis odio. Mauris molestie erat in arcu mattis, at aliquet dolor vehicula. Quisque malesuada \
lectus sit amet nisi pretium, a condimentum ipsum porta. Morbi at dapibus diam. Praesent egestas \
est sed risus elementum, eu rutrum metus ultrices. Etiam fermentum consectetur magna, id rutrum \
felis accumsan a. Aliquam ut pellentesque libero. Sed mi nulla, lobortis eu tortor id, suscipit \
ultricies neque. Morbi iaculis sit amet risus at iaculis. Praesent eget ligula quis turpis \
feugiat suscipit vel non arcu. Interdum et malesuada fames ac ante ipsum primis in faucibus. \
Aliquam sit amet placerat lorem.

Cras a lacus vel ante posuere elementum. Nunc est leo, bibendum ut facilisis vel, bibendum at \
mauris. Nullam adipiscing diam vel odio ornare, luctus adipiscing mi luctus. Nulla facilisi. \
Mauris adipiscing bibendum neque, quis adipiscing lectus tempus et. Sed feugiat erat et nisl \
lobortis pharetra. Donec vitae erat enim. Nullam sit amet felis et quam lacinia tincidunt. Aliquam \
suscipit dapibus urna. Sed volutpat urna in magna pulvinar volutpat. Phasellus nec tellus ac diam \
cursus accumsan.

Nam lectus enim, dapibus non nisi tempor, consectetur convallis massa. Maecenas eleifend dictum \
feugiat. Etiam quis mauris vel risus luctus mattis a a nunc. Nullam orci quam, imperdiet id \
vehicula in, porttitor ut nibh. Duis sagittis adipiscing nisl vitae congue. Donec mollis risus eu \
leo suscipit, varius porttitor nulla porta. Pellentesque ut sem nec nisi euismod vehicula. Nulla \
malesuada sollicitudin quam eu fermentum.";

static LONG_CY: &'static str = "\
Брутэ дольорэ компрэхэнжам йн эжт, ючю коммюны дылыктуч эа, квюо льаорыыт вёвындо мэнандря экз. Ед ыюм емпыдит аккюсам, нык дйкит ютенам ад. Хаж аппэтырэ хонэзтатёз нэ. Ад мовэт путант юрбанйтаж вяш.

Коммодо квюальизквюэ абхоррэант нэ ыюм, праэчынт еракюндйа ылаборарэт эю мыа. Нэ квуым жюмо вольуптатибюж вяш, про ыт бонорюм вёвындо, мэя юллюм новум ку. Пропрёаы такематыш атоморюм зыд ан. Эи омнэжквюы оффекйяж компрэхэнжам жят, апыирёан конкыптам ёнкорруптэ ючю ыт.

Жят алёа лэгыры ед, эи мацим оффэндйт вим. Нык хёнк льаборэж йн, зыд прима тимэам ан. Векж нужквюам инимёкюж ты, ыам эа омнеж ырант рэформйданч. Эрож оффекйяж эю вэл.

Ад нам ножтрюд долорюм, еюж ут вэрыар эюрйпйдяч. Квюач аффэрт тинкидюнт про экз, дёкант вольуптатибюж ат зыд. Ыт зыд экшырки констятюам. Квюо квюиж юрбанйтаж ометтантур экз, хёз экз мютат граэкы рыкючабо, нэ прё пюрто элитр пэрпэтюа. Но квюандо минемум ыам.

Амэт лыгимуз ометтантур кюм ан. Витюпырата котёдиэквюэ нам эю, эю вокынт алёквюам льебэравичсы жят. Экз пыртенакж янтэрэсщэт инзтруктеор нам, еюж ад дйкит каючаэ, шэа витаэ конжтетуто ут. Квюач мандамюч кюм ат, но ёнкорруптэ рэформйданч ючю, незл либриз аюдирэ зыд эи. Ты эож аугюэ иреуры льюкяльиюч, мэль алььтыра докэндё омнэжквюы ат. Анёмал жямиляквюы аккоммодары ыам нэ, экз пэрчёус дэфянятйоныс квюо. Эи дуо фюгит маиорюм.

Эвэртё партйэндо пытынтёюм ыюм ан, шэа ку промпта квюаырэндум. Агам дикунт вим ку. Мюкиуж аюдиам тамквюам про ут, ку мыа квюод квюот эррэм, вяш ад номинави зючкёпит янжольэнж. Нык эи пожжёт путант эффякиантур. Ку еюж нощтыр контынтёонэж. Кюм йужто харюм ёужто ад, ыюм оратио квюоджё экз.

Чонэт факэтэ кюм ан, вэре факэр зальютатуж мэя но. Ыюм ут зальы эффикеэнди, экз про алиё конжыквуюнтюр. Квуй ыльит хабымуч ты, алёа омнэжквюы мандамюч шэа ыт, пльакырат аккюжамюз нэ мэль. Хаж нэ партым нюмквуам прёнкипыз, ат импэрдеэт форынчйбюж кончэктэтюыр шэа. Пльакырат рэформйданч эи векж, ючю дюиж фюйзчыт эи.

Экз квюо ажжюм аугюэ, ат нык мёнём анёмал кытэрож. Кюм выльёт эрюдитя эа. Йн порро малйж кончэктэтюыр хёз, жят кашы эрюдитя ат. Эа вяш мацим пыртенакж, но порро утамюр дяшзынтиыт кюм. Ыт мютат зючкёпит эож, нэ про еракюндйа котёдиэквюэ. Квуй лаудым плььатонэм ед, ку вим ножтрюм лаборамюз.

Вёжи янвыняры хаж ед, ты нолюёжжэ индоктум квуй. Квюач тебиквюэ ут жят, тальэ адхюк убяквюэ йн эож. Ыррор бландит вяш ан, ютроквюы нолюёжжэ констятюам йн ыюм, жят эи прима нобёз тхэопхражтуз. Ты дёкант дэльэнйт нолюёжжэ пэр, молыжтйаы модыратиюз интыллыгам ку мэль.

Ад ылаборарэт конжыквуюнтюр ентырпрытаряш прё, факэтэ лыгэндоч окюррырэт вим ад, элитр рэформйданч квуй ед. Жюмо зальы либриз мэя ты. Незл зюаз видишчы ан ыюм, но пожжэ молыжтйаы мэль. Фиэрэнт адипижкй ометтантур квюо экз. Ут мольлиз пырикюлёз квуй. Ыт квюиж граэко рыпудяары жят, вим магна обльйквюэ контынтёонэж эю, ты шэа эним компльыктётюр.
";

bench_contains_vs_tw!(short_short,
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit.",
    "tis");

// a word with some uncommon letters
bench_contains_vs_tw!(short_word1_long,
    LONG,
    "english");

// a word of only common letters (but does not appear)
bench_contains_vs_tw!(short_word2_long,
    LONG,
    "lite");

bench_contains_vs_tw!(short_1let_long,
    LONG,
    "z");

bench_contains_vs_tw!(short_2let_rare,
    LONG,
    "qq");

bench_contains_vs_tw!(short_2let_common,
    LONG,
    "uu");

bench_contains_vs_tw!(short_3let_long,
    LONG,
    "aga");

bench_contains_vs_tw!(short_1let_cy,
    LONG_CY,
    "Ѯ");

bench_contains_vs_tw!(short_2let_cy,
    LONG_CY,
    "оо");

bench_contains_vs_tw!(short_3let_cy,
    LONG_CY,
    "коэ");

bench_contains_vs_tw!(naive,
    "a".repeat(250),
    "aaaaaaaab");

bench_contains_vs_tw!(naive_rev,
    "a".repeat(250),
    "baaaaaaaa");

bench_contains_vs_tw!(naive_longpat,
    "a".repeat(100_000),
    "a".repeat(24).append("b"));

bench_contains_vs_tw!(naive_longpat_reversed,
    "a".repeat(100_000),
    "b".append(&"a".repeat(24)));

bench_contains_vs_tw!(bb_in_aa,
    "a".repeat(100_000),
    "b".repeat(100));

bench_contains_vs_tw!(aaab_in_aab,
    "aab".repeat(100_000),
    "aaab".repeat(100));

bench_contains_vs_tw!(periodic2,
    "bb".append(&"ab".repeat(99)).repeat(100),
    "ab".repeat(100));

bench_contains_vs_tw!(periodic5,
    "bacba".repeat(39).append("bbbbb").repeat(40),
    "bacba".repeat(40));

// This one is two-way specific
bench_contains_vs_tw!(pathological_two_way,
    "dac".repeat(20_000),
    "bac");

// This one is two-way specific
bench_contains_vs_tw!(pathological_two_way_rev,
    "cad".repeat(20_000),
    "cab");

bench_contains_vs_tw!(bbbaaa,
    "aab".repeat(100_000),
    "b".repeat(100) + &"a".repeat(100));

bench_contains_vs_tw!(aaabbb,
    "aab".repeat(100_000),
    "a".repeat(100) + &"b".repeat(100));

bench_contains_vs_tw!(allright,
    "allrightagtogether".repeat(10_000),
     "allrightaltogether");

bench_contains_vs_tw!(gllright,
    "gllrightaltogether".repeat(10_000),
     "allrightaltogether");


/*
bench_contains_vs_tw!(long_prefix,
    (0..20_000).map(|_| "cad").collect::<String>(),
    (0..100).map(|_| "cad").collect::<String>());
    */

/*
bench_contains_vs_tw!(pathological_test1,
    (0..10_000).map(|_| "daaaaaaaaacc").collect::<String>(),
    (0..100).map(|_| "eaaaaaaaaacc").collect::<String>());
    */

/*
// This one is two-way specific
bench_contains_vs_tw!(long_trim,
    (0..20_000).map(|_| "abcd").collect::<String>(),
    "abc");
    */

#[bench]
pub fn find_char_1(b: &mut Bencher) {
    let haystack = black_box(LONG);
    let needle = black_box('z');
    b.iter(|| {
        let t = haystack.find(needle);
        t
    });
    b.bytes = haystack.len() as u64;
}

#[bench]
pub fn find_char_2(b: &mut Bencher) {
    let haystack = black_box(LONG);
    let needle = black_box('ö');
    b.iter(|| {
        let t = haystack.find(needle);
        t
    });
    b.bytes = haystack.len() as u64;
}

#[bench]
pub fn find_char_3(b: &mut Bencher) {
    let haystack = black_box(LONG);
    let needle = black_box('α');
    b.iter(|| {
        let t = haystack.find(needle);
        t
    });
    b.bytes = haystack.len() as u64;
}

#[bench]
pub fn rfind_char_1(b: &mut Bencher) {
    let haystack = black_box(LONG);
    let needle = black_box('z');
    b.iter(|| {
        let t = haystack.rfind(needle);
        t
    });
    b.bytes = haystack.len() as u64;
} 

#[cfg(feature = "test-set")]
fn bench_data() -> Vec<u8> { vec![0u8; 256 * 1024] }

#[cfg(feature = "test-set")]
#[bench]
pub fn rfind_byte_1(b: &mut Bencher) {
    let haystack = black_box(bench_data());
    let needle = black_box('x');
    b.iter(|| {
        let t = ::twoway::set::rfind_byte(needle as u8, &haystack);
        t
    });
    b.bytes = haystack.len() as u64;
}

#[cfg(feature = "test-set")]
#[bench]
pub fn find_byte_1(b: &mut Bencher) {
    let haystack = black_box(bench_data());
    let needle = black_box('x');
    b.iter(|| {
        let t = ::twoway::set::find_byte(needle as u8, &haystack);
        t
    });
    b.bytes = haystack.len() as u64;
}
