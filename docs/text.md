# Text modality

**Inputs:** bare UTF-8 text (no container magic) and `pdf` — the PDF
lane extracts the document's text layer and feeds it to the same text
pipeline; the container bytes are never hashed.

**Tier 1** canonicalizes first: NFC, lowercase, trailing whitespace
stripped, exactly one `\n` appended — then SHA-256 of the UTF-8 result.
NFC and NFD spellings of the same text hash identically.

**Tier 2** is a 128-word MinHash over FNV-1a hashes of consecutive
3-word shingles of the canonical text (documents under 3 words use
`k = n`). Distance is the Jaccard estimate; `≥ 0.8` is the advisory
match bound.

**Tier 3** is the docx lane: zip container + `word/document.xml` text —
it shares this tier-2 signature rather than defining its own.

```bash
modhash hash readme.md
modhash describe paper.pdf
modhash match draft-v1.md draft-v2.md
```
