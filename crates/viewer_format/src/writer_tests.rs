use crate::test_support::v8_pair;

#[test]
fn v8_split_writer_produces_a_primary_and_an_attribute_file() {
    let (primary, attributes) = v8_pair();
    assert!(!primary.is_empty());
    assert!(!attributes.is_empty());
}
