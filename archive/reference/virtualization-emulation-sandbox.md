# 虚拟化 / 仿真 / 模拟 / 沙箱 概念彻底梳理

> 背景:为在 Apple Silicon(ARM)上用 UTM 跑旧 x86 Windows 复现 minicon 的
> console-agent 中文渲染 bug,顺手把这一族容易混淆的概念系统化。术语保留英文原名,
> 因为中文译名常互相打架(emulation/simulation 都译"仿真/模拟")。

---

## 0. 一句话先分清四个核心词

| 概念 | 目标 | 关键判据 | 典型代价 |
|---|---|---|---|
| **Virtualization(虚拟化)** | 把**真实硬件**切成多份,让多个客户机**原生执行** | 同架构、指令**不翻译**,hypervisor 只拦截特权指令 | 接近原生速度 |
| **Emulation(仿真)** | 用软件**忠实再现**另一套硬件/ISA,好让它的**真实二进制**能跑 | 能运行**为别的架构**编译的真实软件;靠**翻译/解释**指令 | 慢(跨架构可慢 5~30 倍) |
| **Simulation(模拟)** | **建模**一个系统的**行为**以供研究/预测 | 目的是"算出它会怎样",**不追求跑真实二进制**,常是抽象/近似 | 取决于精度,可快可慢 |
| **Sandbox(沙箱)** | **约束**真实代码能碰到的资源(syscall/文件/网络) | 关乎**权限最小化/隔离**,与"跑什么架构"正交 | 很小(主要是策略检查) |

**最容易混的两对:**
- **Emulation vs Simulation**:emulator 追求"**可替代性**"——把真硬件换成它,真软件照跑(QEMU 让 ARM 机器跑 x86 Linux)。simulator 追求"**行为建模**"——预测/分析系统怎么动,不一定能跑真二进制(SPICE 电路仿真、ns-3 网络仿真、飞行模拟器)。口诀:**仿真替代硬件,模拟研究行为。**
- **Virtualization vs Emulation**:同架构、指令直接在真 CPU 上跑 = 虚拟化(快);跨架构、指令要翻译 = 仿真(慢)。**x86-on-ARM 一定是仿真,不管你用什么工具。**

---

## 1. 谱系:从"直接跑"到"完全假装"

```
真实执行 ────────────────────────────────────────────► 完全软件再现
   │                │                 │                      │
 裸机          虚拟化              翻译/仿真               解释执行
 (native)   (same-ISA,HW辅助)   (cross-ISA,JIT翻译)     (逐条解释)
   │                │                 │                      │
 你的程序     KVM/HVF/VMware/     Rosetta2 / QEMU-TCG /   老式解释型
              VirtualBox/Xen      安卓模拟器              模拟器/SPICE
```

横轴是"离真实硬件多远"。越左越快越受限于同架构;越右越慢越自由(能跑任何架构、甚至不存在的硬件)。

**正交的一维:隔离(isolation)。** 沙箱和容器不在上面这条"要不要翻译指令"的轴上,它们回答的是另一个问题:"这段**原生**代码被允许碰到多少东西"。所以可以组合:一个 VM 里再跑容器、容器里再上 seccomp 沙箱。

---

## 2. 逐个概念讲透

### 2.1 Virtualization(虚拟化)
把一台物理机的 CPU/内存/设备**分区**给多个隔离的客户机,客户机指令**原生**跑在真 CPU 上,只有特权/敏感指令被 **hypervisor** 拦截并模拟。前提是**客户机与宿主同架构**。

- **硬件辅助(hardware-assisted)**:Intel **VT-x** / AMD **AMD-V** / Arm **VHE** / Apple **Hypervisor.framework**。CPU 出了个"客户机模式",让绝大多数指令直接跑、只在需要时陷入(VM exit)。这是现代虚拟化快的根本。
- **Type-1 hypervisor(裸机)**:直接装在硬件上——**Xen、Hyper-V、VMware ESXi、KVM**(KVM 是 Linux 内核模块,把 Linux 变成 type-1)。
- **Type-2 hypervisor(宿主型)**:跑在一个普通 OS 之上——**VirtualBox、VMware Workstation/Fusion、Parallels**。
- **Paravirtualization(半虚拟化)**:客户机**知道**自己在 VM 里,主动用高效的 hypercall/virtio 设备,而不是让 hypervisor 硬模拟真实网卡/磁盘。**virtio**(网卡/磁盘/串口)就是这套,QEMU/KVM/UTM 广泛用。
- **OS-level virtualization(操作系统级 = 容器)**:**不开新内核**,多个用户态环境共享宿主内核,靠 Linux **namespaces**(隔离 PID/网络/挂载/用户)+ **cgroups**(限额 CPU/内存)隔离。**Docker/Podman/LXC** 属此类。轻、快、密度高,但**共享内核 = 隔离弱于 VM**,且只能跑同内核族(Linux 容器要 Linux 内核)。

### 2.2 Emulation(仿真)
用软件**再现另一套机器**(通常是不同 ISA 或不存在的老硬件),使**为那套机器编译的真实二进制**能运行。核心是**指令翻译**:
- **解释(interpret)**:逐条读客户机指令、用宿主代码模拟其效果。简单但慢。
- **动态二进制翻译(DBT / JIT)**:把客户机指令块一次性翻译成宿主指令并缓存复用。QEMU 的 **TCG**、Apple **Rosetta 2**、安卓模拟器都靠它。快很多,但仍远慢于原生(跨架构)。
- 典型:**QEMU 全系统仿真**(ARM 上跑 x86 Linux/Windows)、游戏机模拟器、**Rosetta 2**(x86-64 macOS 程序翻译到 Apple Silicon——它是"翻译式仿真",不是虚拟化)、**QEMU-user**(只翻译用户态二进制 + 转发 syscall)。
- 关键点:**同架构也可以"仿真"**(纯软件不用硬件辅助),但那没意义、慢;仿真真正的价值在**跨架构**。

### 2.3 Simulation(模拟)
为**理解/预测**一个系统的行为而**建模**,不以"跑真实软件"为目标,常是更高抽象、可近似:
- **SPICE**(电路)、**ns-3 / OMNeT++**(网络)、**gem5**(计算机体系结构研究——注意 gem5 也能"仿真"跑真二进制,是模拟与仿真的交界)、飞行/物理模拟器、离散事件模拟。
- 与仿真的界线:模拟器问"**这个系统会表现出什么**"(可能用统计/抽象模型);仿真器问"**我能不能忠实到让它的真实客户软件察觉不出差别**"。gem5、cycle-accurate CPU 模型这类"周期精确模拟"骑在两者之间。

### 2.4 Sandbox(沙箱)
让**原生代码**在**受限权限**下运行的隔离机制,目的在**安全/最小权限**,与架构无关:
- **内核机制**:Linux **seccomp-bpf**(过滤 syscall)、**namespaces**、**Landlock**;macOS **App Sandbox / Seatbelt**;Windows **AppContainer / Job Objects**(minicon 就用 Job Object 的 `KILL_ON_JOB_CLOSE` 收束 shell 进程树)。
- **用户态内核型沙箱**:**gVisor**(用 Go 写的用户态内核接管客户 syscall + seccomp 兜底)、**Firecracker / Kata**(极简 microVM,把"轻量"和"VM 级隔离"结合——沙箱与虚拟化的交界)。
- **进程/语言级**:浏览器多进程沙箱、Wasm 运行时(默认无 syscall、能力式)、**Firejail**、chroot(最弱)。
- 沙箱和 VM/容器**可叠加**:容器 + seccomp + 只读 rootfs 是常见组合。

### 2.5 容易被误当"仿真"的三类兼容层(其实都不是仿真)
- **Wine**:"**W**ine **I**s **N**ot an **E**mulator"。它在**同架构**上**重新实现 Windows API/ABI**,把 Win32 调用翻译成宿主(Linux/mac)调用。不翻译指令、不跑客户内核 → 不是仿真、不是 VM,是**兼容层**。
- **WSL1**:把 Linux syscall **翻译**成 Windows NT 内核调用的**兼容层**(同 x86,无指令翻译,无 Linux 内核)。**WSL2** 则相反——是**真·轻量 VM**(Hyper-V 跑真 Linux 内核)。同名两代,机制完全不同。
- **Rosetta 2**:是仿真(指令翻译),但只翻**用户态 x86-64**,不模拟整机;所以它比 QEMU 全系统仿真快得多(还有 Apple 的硬件小助攻,如 TSO 内存序开关)。

### 2.6 容器与镜像(Container & Image)深入

容器是**操作系统级虚拟化**的落地形态,值得单独讲透,因为它和"镜像"这对词最常被混。

**容器到底是什么?** 不是"轻量虚拟机",而是**一个(或一组)被隔离和限额的宿主进程**。它靠三样宿主内核能力拼出来:
- **namespaces(隔离视图)**:PID/network/mount/UTS/IPC/user/cgroup/time 命名空间,让容器内进程只看到自己的进程号、网卡、挂载、主机名……
- **cgroups(资源限额)**:限制/计量 CPU、内存、IO、PID 数。
- **能力裁剪 + 沙箱**:Linux capabilities、seccomp-bpf、LSM(AppArmor/SELinux)收窄它能做什么。
> 所以:容器 = **共享宿主内核** + namespaces 隔离 + cgroups 限额 + 沙箱收权。**不开新内核**,这是它和 VM 的根本区别,也是它轻/快/密度高、但**隔离弱于 VM**(内核是共享攻击面)的原因。

**镜像 vs 容器(最关键的一对):**
- **Image(镜像)** = **不可变的模板**:一层层只读的文件系统层 + 一份配置(入口命令、环境变量、默认用户等)。内容寻址(每层有 sha256 摘要),可被 tag(如 `alpine:3.20`)。
- **Container(容器)** = 镜像的一次**运行实例**:在镜像的只读层之上叠一个**可写层**(改动只写这层,copy-on-write),再套上 namespaces/cgroups 跑起来。
- 类比:**镜像 : 容器 = 类 : 对象 = 磁盘上的程序 : 运行中的进程 = 菜谱 : 做出来的菜**。一个镜像能开无数容器,各自独立可写层。

**分层文件系统(为什么镜像能高效复用):**
- 用 **union / overlay 文件系统**(overlayfs)把多只读层 + 一可写层叠成一个视图。
- 层是**内容寻址、可共享**的:多个镜像共用同一基础层(如都基于 `debian`),磁盘和拉取都省。
- 构建(Dockerfile/Containerfile)每条指令产生一层;改动只重建受影响的层(层缓存)。多阶段构建(multi-stage)只把产物拷进最终镜像,减小体积。

**标准与运行时分层(生态其实很规整):**
- **OCI 标准**(Open Container Initiative):**Image Spec**(镜像格式)、**Runtime Spec**(怎么跑一个容器)、**Distribution Spec**(仓库怎么推拉)。正因有标准,Docker 造的镜像 Podman/containerd 都能跑。
- **低层运行时**:**runc**(参考实现)、**crun**(C 写、更快)、**gVisor/runsc**、**Kata**(每容器套 microVM,强隔离)。
- **高层运行时/引擎**:**containerd**、**CRI-O**(K8s 用);**Docker Engine** = `dockerd` + containerd + runc 的组合。
- **仓库(registry)**:Docker Hub、GHCR、Quay……镜像 = manifest(清单)+ config + 各层 blob,按摘要推拉。

**Docker vs Podman(常问):**
- **Docker**:有常驻**守护进程 `dockerd`**(root),CLI 通过它干活;生态最广。
- **Podman**:**无守护进程(daemonless)**、**默认 rootless**(靠 user namespace 让普通用户跑容器)、原生 **pod** 概念(一组共享网络的容器,贴合 K8s);CLI 基本兼容 `docker`(可 `alias docker=podman`)。
- **containerd/nerdctl、LXC/LXD** 是另外的选择;K8s 通过 **CRI** 接 containerd/CRI-O,**不再直接用 Docker**。

**一个反直觉但重要的点:Mac/Windows 上"没有原生 Linux 容器"。**
- Linux 容器要 Linux 内核。macOS/Windows 上的 **Docker Desktop / Podman machine / colima / lima / Rancher Desktop** 其实**先起一个轻量 Linux VM**,容器都跑在那台 VM 里。所以"Mac 上跑 Docker" = **虚拟化(VM)+ 容器** 叠加——又绕回第 1 章的谱系。
- **Windows 容器**另说:它共享 **Windows** 内核,分**进程隔离**(process-isolated,共享内核)与 **Hyper-V 隔离**(每容器套一个极简 VM,隔离更强)两种模式。

**"镜像"这个词的两种含义(中文尤其易混):**
- **VM 磁盘镜像**(disk image):VMDK/VHD/VDI/qcow2/ISO —— 一整块虚拟磁盘或光盘(见第 4、6 节我们搬的 IE11-Win7)。
- **容器镜像**(container image):OCI 分层文件系统 + 配置 —— 上面讲的这套。
- 两者都叫"镜像",但**一个是整盘、一个是分层应用打包**,机制与用途完全不同。别混。

---

## 3. 工具落位表

| 工具 | 类别 | 跨架构? | 机制 | 宿主 | 备注 |
|---|---|---|---|---|---|
| **KVM** | 虚拟化(type-1,内核模块) | 否 | HW 辅助(VT-x/AMD-V) | Linux | 常配 QEMU 当设备模型 |
| **Xen** | 虚拟化(type-1) | 否 | 全虚 + 半虚 | 裸机 | 云上老牌 |
| **VMware ESXi** | 虚拟化(type-1) | 否 | HW 辅助 | 裸机 | 企业数据中心 |
| **Hyper-V** | 虚拟化(type-1) | 否 | HW 辅助 | Windows | WSL2/沙箱底座 |
| **VMware Workstation/Fusion** | 虚拟化(type-2) | 否 | HW 辅助 | Win/Linux/mac | Fusion 现支持 Apple 芯片跑 ARM 客户机 |
| **VirtualBox** | 虚拟化(type-2) | 否(主要) | HW 辅助 + 少量软件 | x86 宿主为主 | ARM 版仍预览;**自带完整引擎** |
| **QEMU** | **仿真 + 虚拟化** | **是**(TCG 模式) | TCG 动态翻译 或 接 KVM/HVF 加速 | 全平台 | 一身二用:纯仿真 or 当加速前端 |
| **UTM** | **前端/编排器** | 是(靠 QEMU) | 底层 = QEMU 或 Apple Virtualization.framework | macOS/iOS | **不是引擎**,是壳 |
| **Apple Virtualization.framework** | 虚拟化 | 否 | Hypervisor.framework | Apple 芯片 | 跑 ARM Linux/macOS,快 |
| **Rosetta 2** | 仿真(用户态翻译) | 是(x86→ARM) | DBT + 硬件助攻 | Apple 芯片 | 只翻用户态 |
| **Docker / Podman / LXC** | 容器(OS 级虚拟化) | 否 | namespaces + cgroups | Linux | 共享内核;Docker 有守护进程,Podman daemonless/rootless |
| **containerd / CRI-O** | 容器高层运行时 | 否 | 管镜像/生命周期,调 runc | Linux | K8s 经 CRI 接它,不再直接用 Docker |
| **runc / crun / runsc** | 容器低层运行时 | 否 | 按 OCI Runtime Spec 起容器 | Linux | runsc=gVisor;crun 更快 |
| **Docker Desktop / colima / Podman machine / lima** | 容器 **on VM** | 否 | 先起轻 Linux VM,容器跑里面 | mac/Win | "Mac 上的 Docker" = 虚拟化 + 容器叠加 |
| **gVisor / Firecracker / Kata** | 沙箱↔microVM 交界 | 否 | 用户态内核 / 极简 VMM | Linux | 强隔离 + 轻量 |
| **Wine** | 兼容层(非仿真) | 否 | 重实现 Win32 API | 同架构 | 不跑客户内核 |
| **WSL1 / WSL2** | 兼容层 / 轻 VM | 否 | syscall 翻译 / Hyper-V | Windows | 同名两代机制不同 |

---

## 4. UTM 与 VirtualBox 的关系(本次实践)

- **VirtualBox** = 自带引擎的 type-2 hypervisor;经典在 x86 宿主上用 VT-x 加速。ARM 上帮不了 x86 加速。
- **UTM** = macOS 上的**前端**,底下调 **QEMU**(能仿真、能接 HVF 加速)或 **Apple Virtualization.framework**。它自己不是 hypervisor。
- **联系(为什么能互搬)**:
  - **磁盘格式**互通:VirtualBox 原生 **VDI**,也读写 **VMDK/VHD**;QEMU/UTM 读 **VMDK/VHD/VDI/qcow2**。所以能把 VirtualBox 导出的 VMDK 直接给 UTM/QEMU 跑。
  - **OVA/OVF**:跨管理器搬家的**打包标准**(一个 tar 内含 OVF 硬件描述 + VMDK 磁盘)。
- **坑(虚拟硬件契约)**:客户机里装的驱动**绑定虚拟硬件**(芯片组、IDE/SATA/NVMe 控制器)。搬盘到新宿主若硬件不同,Windows 找不到启动盘驱动 → **0x7B INACCESSIBLE_BOOT_DEVICE 蓝屏**。对策:UTM 配 **i440fx + PIIX3 IDE** 去**匹配 VirtualBox 默认控制器**,让已装驱动仍认得盘。
- **本次为什么慢**:M4 是 ARM,跑 x86 Windows 只能 **QEMU-TCG 仿真**(无硬件加速)——UTM/VirtualBox 都逃不掉这条物理。所以我们优先找**预装好的镜像**(跳过更慢的安装环节)。

---

## 5. "遇到一个新工具怎么快速定位"四问

1. **同架构还是跨架构?** 跨架构 → 一定含仿真(慢);同架构 → 可能是虚拟化/容器/沙箱/兼容层。
2. **跑不跑一个独立内核?** 跑 → VM(虚拟化或仿真);不跑、共享宿主内核 → 容器或兼容层。
3. **翻不翻译指令?** 翻译 → 仿真;不翻译、原生执行 → 虚拟化/容器/沙箱。
4. **主要目的是"跑得动别的软件"还是"限制它能干嘛"还是"研究它会怎样"?** 分别对应 仿真/虚拟化、沙箱、模拟。

> 例:WSL2 → 同架构、跑独立 Linux 内核、不翻译指令、目的是跑 Linux 软件 = **轻量虚拟化**。
> Docker(Linux 上)→ 同架构、共享内核、不翻译 = **容器(OS 级虚拟化)**。
> QEMU 在 ARM 上跑 x86 → 跨架构、翻译指令 = **仿真**。

---

## 6. 与 minicon 的关系(为什么这份笔记在这里)

- minicon 是纯软件的终端(自绘光栅),它**本身不虚拟化/不仿真**——但它要在**六格**(win/mac/linux × x86/arm)上验证,而在开发机(ARM Mac)上覆盖 x86 目标就得靠 UTM/QEMU **仿真**,慢是本质。
- 复现旧 Windows(build 14393,无 ConPTY)的 console-agent 中文乱码,需要一台**旧控制台**的 Windows;真机稀缺,故用 UTM 仿真 + 预装镜像。
- 这也解释了 minicon 的"六格"跨平台策略里,x86-on-ARM 一路为何总是最慢、最脆的一环。

---

*本文档为概念参考,不含产品承诺;术语以英文为准。*
