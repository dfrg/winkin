//! Caller offset tests. They pin positions as places in the caller's
//! nodes and text, both ways.

use super::*;

/// A position converts to a node and an offset in its text, and back.
#[test]
fn positions_are_places_in_the_callers_text() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "one   ");
        b.open_box(NodeKey(2), &ahem(), None);
        b.text(NodeKey(3), "two");
        b.close_box();
        b.text(NodeKey(4), " three");
    });
    assert_eq!(layout.text(), "one two three");
    for (offset, affinity) in [(1, Affinity::Downstream), (3, Affinity::Upstream)] {
        let position = layout.position(NodeKey(3), offset, affinity).unwrap();
        let node = layout.node_position(position).unwrap();
        assert_eq!((node.key, node.offset), (NodeKey(3), offset));
    }
    // Collapsed white space at a node's end is on that node's side.
    assert_eq!(
        layout.position(NodeKey(1), 5, Affinity::Downstream),
        Some(Position::new(4, Affinity::Upstream))
    );
    let before = layout
        .node_position(Position::new(4, Affinity::Upstream))
        .unwrap();
    assert_eq!((before.key, before.offset), (NodeKey(1), 4));
    let after = layout.node_position(Position::from(4)).unwrap();
    assert_eq!((after.key, after.offset), (NodeKey(3), 0));
    // A first letter is its text node's.
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.set_first_letter(NodeKey(9), &styled(|style| style.font.size = 40.0), None);
        b.text(NodeKey(1), "Hello");
    });
    let position = layout
        .position(NodeKey(1), 3, Affinity::Downstream)
        .unwrap();
    assert_eq!(position, Position::from(3));
    let node = layout.node_position(position).unwrap();
    assert_eq!((node.key, node.offset), (NodeKey(1), 3));
    // Without the map, there is nothing to convert by.
    let mut cx = context();
    let mut plain = Layout::new();
    let mut b = plain.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&ahem()),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "abc");
    b.finish(&mut cx);
    assert_eq!(plain.node_position(Position::from(1)), None);
}
