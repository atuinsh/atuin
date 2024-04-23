use comrak::{
    arena_tree::NodeEdge,
    nodes::{AstNode, NodeValue},
    parse_document, Arena, ComrakOptions,
};

#[derive(Debug, Clone)]
pub struct Block {
    pub info: String,
    pub code: String,
}

// why yes, this is stolen from the examples:
// https://github.com/kivikakk/comrak/blob/56581d7275d8180f1a55771a2f3d41b6ebef26a6/examples/traverse_demo.rs
fn extract_text_traverse<'a>(root: &'a AstNode<'a>) -> Vec<Block> {
    let mut output_blocks = Vec::new();

    // Use `traverse` to get an iterator of `NodeEdge` and process each.
    for edge in root.traverse() {
        if let NodeEdge::Start(node) = edge {
            if let NodeValue::CodeBlock(ref block) = node.data.borrow().value {
                let block = Block {
                    code: block.literal.clone(),
                    info: block.info.clone(),
                };

                output_blocks.push(block);
            }
        }
    }

    output_blocks
}

pub fn parse(md: &str) -> Vec<Block> {
    // setup parser
    let arena = Arena::new();
    let options = ComrakOptions::default();

    // parse document and return root.
    let root = parse_document(&arena, md, &options);

    // extract text and print
    extract_text_traverse(root)
}

#[cfg(test)]
mod test {
    use crate::markdown::parse;

    // Test if we can parse some markdown containing a single code block
    #[test]
    fn test_parse_simple() {
        let md = "
```bash
echo 'foo bar'
```
";

        let blocks = parse(md);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].info, "bash");
        assert_eq!(blocks[0].code, "echo 'foo bar'\n");
    }

    #[test]
    fn test_parse_not_so_simple() {
        let md = "
# Hello I am a doc

## Here is how you do some things

### This thing is really cool
```bash
echo 'foo bar'
```

### This thing is cooler and with a better shell

```zsh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install atuin
```
";

        let blocks = parse(md);

        assert_eq!(blocks.len(), 2);

        assert_eq!(blocks[0].info, "bash");
        assert_eq!(blocks[0].code, "echo 'foo bar'\n");

        assert_eq!(blocks[1].info, "zsh");
        assert_eq!(
            blocks[1].code,
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh\ncargo install atuin\n"
        );
    }
}
