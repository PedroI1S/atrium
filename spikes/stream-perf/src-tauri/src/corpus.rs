//! Deterministic ASCII markdown used in bench mode.
//!
//! M0-05's fake agent streams `x` repeated, which exercises neither the markdown parser nor the
//! layout engine. In bench mode the bridge swaps each delta's text for a slice of this corpus of the
//! **same length**, so the bytes on the socket, the number of events and the token rate are exactly
//! what the daemon produced, while the webview renders realistic markdown (headings, lists, code
//! fences, tables, inline code, links). Every slice of a message starts at that message's running
//! offset, so a message's deltas concatenate to a prefix of the corpus.

pub struct Corpus(Vec<u8>);

const BLOCKS: &[&str] = &[
    "## Plan for the streaming path\n\n",
    "The path is **daemon, unix socket, Rust client, webview**. Every hop is bounded: a slow consumer pushes back on the socket instead of growing a queue, and the *applied* cursor only moves once a frame has been painted. See [the spike report](https://example.com/spike) for the numbers.\n\n",
    "- batch deltas once per animation frame\n- re-parse only the `open block`\n- virtualize rows that are far from the viewport\n- keep the **composer** responsive\n\n",
    "```rust\nfn apply(batch: &[Event], state: &mut Transcript) {\n    for event in batch {\n        state.push(event);\n    }\n    state.mark_dirty();\n}\n```\n\n",
    "Tokens arrive in tiny pieces, so a word such as `reconciliation` can be split across three deltas. The parser must never treat half of a **bold run or a `code span` as a finished block, and it must not flicker when the closing marker arrives.\n\n",
    "1. read the cursor from the store\n2. replay everything after it\n3. dedupe `id <= applied` in the webview\n4. acknowledge after paint\n\n",
    "> A transcript of a million tokens is only a few megabytes of text. The cost is in the DOM, not in the string.\n\n",
    "| encoding | bytes per delta | decode |\n|---|---|---|\n| json | 180 | parse |\n| msgpack | 120 | decode |\n| bin | 40 | view |\n\n",
    "### Findings so far\n\n",
    "The tool call finished in 412 ms and returned 38 lines. Nothing in the output needed the user's attention, so the agent moved on to the next file and continued with the refactor, keeping the public signatures stable and the tests green after each step.\n\n",
    "```ts\nconst frame = (t: number) => {\n  flush(pending);\n  pending = [];\n  requestAnimationFrame(frame);\n};\n```\n\n",
    "Short paragraph.\n\n",
    "Another longer paragraph with `inline code`, a [link](https://example.org/a/b?c=d), and some *emphasis* so that the incremental parser sees a realistic mix of inline syntax while text is still arriving one token at a time, which is the hardest case for any renderer that re-parses the open block on every frame.\n\n",
];

impl Corpus {
    pub fn new() -> Self {
        let mut one = String::new();
        for b in BLOCKS {
            one.push_str(b);
        }
        assert!(one.is_ascii());
        Corpus(one.into_bytes())
    }

    /// `len` ASCII bytes starting at `offset` (wrapping around the corpus).
    pub fn slice(&self, offset: usize, len: usize) -> String {
        let n = self.0.len();
        let mut out = Vec::with_capacity(len);
        let mut at = offset % n;
        while out.len() < len {
            let take = (len - out.len()).min(n - at);
            out.extend_from_slice(&self.0[at..at + take]);
            at = 0;
        }
        String::from_utf8(out).expect("corpus is ASCII")
    }
}

impl Default for Corpus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_concatenate_to_a_prefix() {
        let c = Corpus::new();
        let whole = c.slice(0, 100_000);
        let mut joined = String::new();
        let mut off = 0;
        for len in [4, 7, 1, 4000, 33] {
            joined.push_str(&c.slice(off, len));
            off += len;
        }
        assert_eq!(joined, whole[..off]);
        assert_eq!(whole.len(), 100_000);
    }
}
