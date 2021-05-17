use itertools::Itertools;

#[test]
fn specialization_intersperse() {
    let mut iter = (1..2).intersperse(0);
    iter.clone().for_each(|x| assert_eq!(Some(x), iter.next()));

    let mut iter = (1..3).intersperse(0);
    iter.clone().for_each(|x| assert_eq!(Some(x), iter.next()));

    let mut iter = (1..4).intersperse(0);
    iter.clone().for_each(|x| assert_eq!(Some(x), iter.next()));
}
