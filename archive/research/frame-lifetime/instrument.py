from pathlib import Path
import shutil,difflib,subprocess
root=Path.cwd();base=root/'target/font-platform-fix/third_party/softbuffer/src/backends/cg.rs';p=root/'target/frame-lifetime/softbuffer/src/backends/cg.rs';old=subprocess.check_output(['git','-C',str(root/'target/font-platform-fix'),'show','8d8de88c:third_party/softbuffer/src/backends/cg.rs'],text=True);s=old
s=s.replace('use crate::backend_interface::*;', 'use crate::backend_interface::*;\nmod frame_trace;')
s=s.replace('struct MappedPixels(memmap2::MmapMut);','''struct MappedPixels(Option<memmap2::MmapMut>);
impl Drop for MappedPixels {
    fn drop(&mut self) {
        if let Some(mapping) = &self.0 { frame_trace::releasing(mapping.as_ptr() as usize); }
    }
}
struct ProviderMapping(memmap2::Mmap);
impl Drop for ProviderMapping {
    fn drop(&mut self) { frame_trace::releasing(self.0.as_ptr() as usize); }
}''')
s=s.replace('.map(Self)', '''.map(|mapping| {
                frame_trace::allocated(mapping.as_ptr() as usize, mapping.len());
                Self(Some(mapping))
            })''')
s=s.replace('fn into_data_provider(self)', 'fn into_data_provider(mut self)')
s=s.replace('info.cast::<memmap2::Mmap>()','info.cast::<ProviderMapping>()')
s=s.replace('let mapping = self.0.make_read_only()', 'let mapping = self.0.take().expect("mapping");\n        let mapped_address = mapping.as_ptr() as usize;\n        let mapping = mapping.make_read_only()')
s=s.replace('SoftBufferError::PlatformError(Some(format!("freeze pixel mapping: {error}")), None)', 'frame_trace::releasing(mapped_address);\n            SoftBufferError::PlatformError(Some(format!("freeze pixel mapping: {error}")), None)')
s=s.replace('let owner = Box::into_raw(Box::new(mapping));','frame_trace::event("freeze", mapping.as_ptr() as usize);\n        let owner = Box::into_raw(Box::new(ProviderMapping(mapping)));')
s=s.replace('self.0.as_ptr()', 'self.0.as_ref().expect("mapping").as_ptr()')
# ProviderMapping is not optional.
s=s.replace('frame_trace::releasing(self.0.as_ref().expect("mapping").as_ptr() as usize);','frame_trace::releasing(self.0.as_ptr() as usize);')
s=s.replace('self.0.len()', 'self.0.as_ref().expect("mapping").len()')
s=s.replace('self.0.as_mut_ptr()', 'self.0.as_mut().expect("mapping").as_mut_ptr()')
s=s.replace('let data_provider = self.buffer.into_data_provider()?;', 'let traced_address = self.buffer.0.as_ref().expect("mapping").as_ptr() as usize;\n        frame_trace::event("present_begin", traced_address);\n        let data_provider = self.buffer.into_data_provider()?;')
s=s.replace('CATransaction::begin();', 'frame_trace::event("image_created", traced_address);\n        CATransaction::begin();')
s=s.replace('CATransaction::commit();\n        Ok(())', 'frame_trace::event("layer_set_contents", traced_address);\n        CATransaction::commit();\n        frame_trace::event("transaction_commit", traced_address);\n        drop(image);\n        drop(data_provider);\n        frame_trace::event("locals_dropped", traced_address);\n        Ok(())')
p.write_text(s)
module=p.parent/'cg';module.mkdir(exist_ok=True);shutil.copyfile(root/'archive/research/frame-lifetime/frame_trace.rs',module/'frame_trace.rs')
(root/'archive/research/frame-lifetime/instrumentation.patch').write_text(''.join(difflib.unified_diff(old.splitlines(True),s.splitlines(True),fromfile='a/src/backends/cg.rs',tofile='b/src/backends/cg.rs')))
