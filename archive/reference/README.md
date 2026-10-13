# reference/

概念与背景参考文档(非产品承诺、非 PRD)。工程中反复用到、值得系统化沉淀的知识放这里。

## 索引

- [虚拟化 / 仿真 / 模拟 / 沙箱 概念彻底梳理](virtualization-emulation-sandbox.md)
  —— emulation / simulation / virtualization / sandbox 的分界;容器与镜像深入
  (Docker/Podman/OCI/runc、镜像 vs 容器、分层文件系统);VMware/QEMU/VirtualBox/UTM
  等工具落位表;VirtualBox↔UTM 关系与 0x7B 坑;"遇到新工具怎么定位"四问。
  背景:在 Apple Silicon 上用 UTM 仿真旧 x86 Windows 复现 minicon 的 console-agent
  中文渲染 bug。
