
## 注意

现在需要把以下改动patch到 ~/.cargo/registry/src/

https://github.com/pengzechen/page_table_multiarch/commit/5aabe07831b586904632abe44cdfc6ee6f3e5b44

```bash
chmod +x rootfs/patch.sh 
./rootfs/patch.sh 
```

expect out put:

✅ Found:
  /home/ajax/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/page_table_entry-0.5.7/src/arch/riscv.rs
📦 Backup created: /home/ajax/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/page_table_entry-0.5.7/src/arch/riscv.rs.bak
✅ Replacement done