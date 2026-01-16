

## BusyBox 
1、版本 busybox-1.37.0.tar.bz2 (https://busybox.net/downloads/busybox-1.37.0.tar.bz2)
2、工具链 riscv64-linux-musl-
3、编译选项 static ; 去掉 SH1 SH256

```bash
make ARCH=riscv CROSS_COMPILE=riscv64-linux-musl- CONFIG_PREFIX=$(pwd)/_install install
```

## 文件系统制作
1、修改rootfs.sh: BUSY_BOX_DIR
2、执行sudo ./rootfs.sh

## 文件系统使用

目前没有sd卡驱动，使用ramfs。需要将文件系统加载到指定的ram.
注意：0x8900_0000 - 0x8fe0_0000 大概是100Mb，所以目前文件系统不能超过100Mb
```bash
fatload mmc 0:1 0x89000000  ext4_100m.img
```