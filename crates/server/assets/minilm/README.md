# Local MiniLM artifacts

`minilm.json` pins Qdrant's Apache-2.0 ONNX export of
`sentence-transformers/all-MiniLM-L6-v2` and the tokenizer/configuration files.
The model weights are provisioned by `scripts/download_embedding_model.py`,
not committed to Git. The script and server verify every SHA-256 digest.

Source: https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/tree/cb27c1f221563ca44e58ad2a694965db39c8153d

The upstream declares Apache-2.0. Preserve this provenance and the accompanying
`LICENSE` when distributing the artifacts. No Cairn user content is needed
for provisioning. Runtime inference runs within `cairn-server` without network.
