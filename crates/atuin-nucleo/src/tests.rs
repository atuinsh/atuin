use std::sync::Arc;

use atuin_nucleo_matcher::Config;

use crate::{pattern, Nucleo};

#[test]
fn active_injector_count() {
    let mut nucleo: Nucleo<()> = Nucleo::new(Config::DEFAULT, Arc::new(|| ()), Some(1), 1);
    assert_eq!(nucleo.active_injectors(), 0);
    let injector = nucleo.injector();
    assert_eq!(nucleo.active_injectors(), 1);
    let injector2 = nucleo.injector();
    assert_eq!(nucleo.active_injectors(), 2);
    drop(injector2);
    assert_eq!(nucleo.active_injectors(), 1);
    nucleo.restart(false);
    assert_eq!(nucleo.active_injectors(), 0);
    let injector3 = nucleo.injector();
    assert_eq!(nucleo.active_injectors(), 1);
    nucleo.tick(0);
    assert_eq!(nucleo.active_injectors(), 1);
    drop(injector);
    assert_eq!(nucleo.active_injectors(), 1);
    drop(injector3);
    assert_eq!(nucleo.active_injectors(), 0);
}

#[derive(Clone, Debug)]
struct TestItem {
    text: String,
    category: u32,
    priority: u32,
}

#[test]
fn filter_excludes_items() {
    let mut nucleo: Nucleo<TestItem> = Nucleo::new(Config::DEFAULT, Arc::new(|| ()), Some(1), 1);
    let injector = nucleo.injector();

    // Add items with different categories
    injector.push(
        TestItem {
            text: "apple".into(),
            category: 1,
            priority: 10,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );
    injector.push(
        TestItem {
            text: "apricot".into(),
            category: 2,
            priority: 20,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );
    injector.push(
        TestItem {
            text: "avocado".into(),
            category: 1,
            priority: 30,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );

    // Search without filter - should get all 3
    nucleo.pattern.reparse(
        0,
        "a",
        pattern::CaseMatching::Ignore,
        pattern::Normalization::Smart,
        false,
    );
    while nucleo.tick(10).running {}
    assert_eq!(nucleo.snapshot().matched_item_count(), 3);

    // Set filter to only include category 1
    nucleo.set_filter(Some(Arc::new(|item: &TestItem| item.category == 1)));

    // Search again - should only get 2 items (apple, avocado)
    while nucleo.tick(10).running {}
    assert_eq!(nucleo.snapshot().matched_item_count(), 2);

    // Verify the items are correct
    let items: Vec<_> = nucleo
        .snapshot()
        .matched_items(..)
        .map(|i| i.data.text.clone())
        .collect();
    assert!(items.contains(&"apple".to_string()));
    assert!(items.contains(&"avocado".to_string()));
    assert!(!items.contains(&"apricot".to_string()));

    // Remove filter - should get all 3 again
    nucleo.set_filter(None);
    while nucleo.tick(10).running {}
    assert_eq!(nucleo.snapshot().matched_item_count(), 3);
}

#[test]
fn scorer_affects_sort_order() {
    let mut nucleo: Nucleo<TestItem> = Nucleo::new(Config::DEFAULT, Arc::new(|| ()), Some(1), 1);
    let injector = nucleo.injector();

    // Add items with different priorities
    injector.push(
        TestItem {
            text: "banana".into(),
            category: 1,
            priority: 10,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );
    injector.push(
        TestItem {
            text: "blueberry".into(),
            category: 1,
            priority: 100,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );
    injector.push(
        TestItem {
            text: "blackberry".into(),
            category: 1,
            priority: 50,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );

    // Search without scorer - results sorted by fuzzy score
    nucleo.pattern.reparse(
        0,
        "b",
        pattern::CaseMatching::Ignore,
        pattern::Normalization::Smart,
        false,
    );
    while nucleo.tick(10).running {}
    assert_eq!(nucleo.snapshot().matched_item_count(), 3);

    // Set scorer that uses priority as the score (ignoring fuzzy score)
    nucleo.set_scorer(Some(Arc::new(|item: &TestItem, _fuzzy_score| {
        item.priority
    })));

    // Search again - should be sorted by priority (high to low)
    while nucleo.tick(10).running {}
    let items: Vec<_> = nucleo
        .snapshot()
        .matched_items(..)
        .map(|i| i.data.clone())
        .collect();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].text, "blueberry"); // priority 100
    assert_eq!(items[1].text, "blackberry"); // priority 50
    assert_eq!(items[2].text, "banana"); // priority 10

    // Verify external_score is set correctly
    let matches = nucleo.snapshot().matches();
    assert_eq!(matches[0].external_score, 100);
    assert_eq!(matches[1].external_score, 50);
    assert_eq!(matches[2].external_score, 10);
}

#[test]
fn filter_and_scorer_combined() {
    let mut nucleo: Nucleo<TestItem> = Nucleo::new(Config::DEFAULT, Arc::new(|| ()), Some(1), 1);
    let injector = nucleo.injector();

    injector.push(
        TestItem {
            text: "cherry".into(),
            category: 1,
            priority: 10,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );
    injector.push(
        TestItem {
            text: "cranberry".into(),
            category: 2,
            priority: 100,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );
    injector.push(
        TestItem {
            text: "coconut".into(),
            category: 1,
            priority: 50,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );

    // Set both filter (category 1) and scorer (priority)
    nucleo.set_filter(Some(Arc::new(|item: &TestItem| item.category == 1)));
    nucleo.set_scorer(Some(Arc::new(|item: &TestItem, _| item.priority)));

    nucleo.pattern.reparse(
        0,
        "c",
        pattern::CaseMatching::Ignore,
        pattern::Normalization::Smart,
        false,
    );
    while nucleo.tick(10).running {}

    // Should have 2 items (cherry, coconut) sorted by priority
    let items: Vec<_> = nucleo
        .snapshot()
        .matched_items(..)
        .map(|i| i.data.clone())
        .collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].text, "coconut"); // priority 50
    assert_eq!(items[1].text, "cherry"); // priority 10
}

#[test]
fn scorer_combines_with_fuzzy_score() {
    let mut nucleo: Nucleo<TestItem> = Nucleo::new(Config::DEFAULT, Arc::new(|| ()), Some(1), 1);
    let injector = nucleo.injector();

    injector.push(
        TestItem {
            text: "date".into(),
            category: 1,
            priority: 100,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );
    injector.push(
        TestItem {
            text: "dragon fruit".into(),
            category: 1,
            priority: 10,
        },
        |item, cols| cols[0] = item.text.clone().into(),
    );

    // Set scorer that combines fuzzy score with priority
    nucleo.set_scorer(Some(Arc::new(|item: &TestItem, fuzzy_score| {
        fuzzy_score + item.priority
    })));

    nucleo.pattern.reparse(
        0,
        "d",
        pattern::CaseMatching::Ignore,
        pattern::Normalization::Smart,
        false,
    );
    while nucleo.tick(10).running {}

    // Both items match, verify that external_score includes priority boost
    let matches = nucleo.snapshot().matches();
    assert_eq!(matches.len(), 2);

    // The raw fuzzy scores should be in Match.score
    // The combined scores should be in Match.external_score
    for m in matches {
        let item = nucleo.snapshot().get_item(m.idx).unwrap();
        assert_eq!(m.external_score, m.score + item.data.priority);
    }
}
