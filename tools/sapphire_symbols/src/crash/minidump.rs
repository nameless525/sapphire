//! A desktop minidump, walked by `minidump-processor` with the store's `.sym` files.

use std::path::PathBuf;

use anyhow::Context as _;
use breakpad_symbols::SimpleModule;
use minidump::{Minidump, MinidumpModuleList, Module};
use minidump_unwind::{
    FileError, FileKind, FillSymbolError, FrameSymbolizer, FrameWalker, SymbolProvider, Symbolizer,
};

use super::{Crash, Frame, Location, Reader, Thread, block_on, leaf};
use crate::store::Packaged;

pub(super) fn read(bytes: Vec<u8>, reader: &Reader<'_>) -> anyhow::Result<Crash> {
    let dump = Minidump::read(bytes).context("not a minidump")?;
    // Every module of ours gets its `.sym` before the walk, which looks them up in the store.
    let modules: MinidumpModuleList = dump.get_stream().context("the minidump lists no modules")?;
    for module in modules.iter() {
        if let Some(id) = module.debug_identifier() {
            reader.module(&module.code_file(), &id.breakpad().to_string())?;
        }
    }
    let provider = Named { symbolizer: &reader.symbolizer, packaged: reader.packaged };
    let state = block_on(minidump_processor::process_minidump(&dump, &provider))
        .context("minidump-processor can't read it")?;

    let mut summary = Vec::new();
    if let Some(exception) = &state.exception_info {
        summary.push(format!("{} at {:#x}", exception.reason, exception.address.0));
    }
    if let Some(assertion) = &state.assertion {
        summary.push(assertion.clone());
    }
    summary.push(format!("{} threads; the crashing thread's frames follow", state.threads.len()));
    let crashed = state.requesting_thread.and_then(|index| state.threads.get(index));
    let threads = crashed
        .map(|stack| Thread {
            name: match &stack.thread_name {
                Some(name) => format!("{} {name}", stack.thread_id),
                None => stack.thread_id.to_string(),
            },
            crashed: true,
            frames: stack
                .frames
                .iter()
                .map(|frame| Frame {
                    address: frame.instruction,
                    module: frame.module.as_ref().map(|module| {
                        let named = provider.renamed(module).and_then(|renamed| renamed.debug_file);
                        let code_file = module.code_file();
                        let name = named.unwrap_or_else(|| leaf(&code_file).to_owned());
                        (name, frame.instruction.wrapping_sub(module.base_address()))
                    }),
                    // `minidump-unwind` lists them innermost first already, as `Frame` does.
                    inlined: frame
                        .inlines
                        .iter()
                        .map(|inline| Location {
                            function: Some(inline.function_name.clone()),
                            file: inline.source_file_name.clone(),
                            line: inline.source_line,
                        })
                        .collect(),
                    location: Location {
                        function: frame.function_name.clone(),
                        file: frame.source_file_name.clone(),
                        line: frame.source_line,
                    },
                    found_by: Some(frame.trust.as_str()),
                })
                .collect(),
        })
        .into_iter()
        .collect();
    Ok(Crash { kind: "minidump", summary, threads })
}

/// The store's symbols, looked up by each module's debug file and ID as the minidump names them, except that a
/// module of ours is named by the debug file its package records: the macOS monitor writes the
/// executable's as `<Unknown>`, since it can't read the main image's path, and its ID alone finds it.
struct Named<'a> {
    symbolizer: &'a Symbolizer,
    packaged: &'a [Packaged],
}

impl Named<'_> {
    /// `module` under its package's debug file, where the minidump names it otherwise.
    fn renamed(&self, module: &(dyn Module + Sync)) -> Option<SimpleModule> {
        let id = module.debug_identifier()?;
        let packaged = self.packaged.iter().find(|packaged| packaged.is(&id.breakpad().to_string()))?;
        let debug_file = &packaged.module.debug_file;
        if module.debug_file().is_some_and(|name| leaf(&name) == debug_file) {
            return None;
        }
        Some(SimpleModule {
            base_address: Some(module.base_address()),
            size: Some(module.size()),
            code_file: Some(module.code_file().into_owned()),
            code_identifier: module.code_identifier(),
            debug_file: Some(debug_file.clone()),
            debug_id: Some(id),
            version: None,
        })
    }
}

#[async_trait::async_trait]
impl SymbolProvider for Named<'_> {
    async fn fill_symbol(
        &self,
        module: &(dyn Module + Sync),
        frame: &mut (dyn FrameSymbolizer + Send),
    ) -> Result<(), FillSymbolError> {
        match self.renamed(module) {
            Some(renamed) => self.symbolizer.fill_symbol(&renamed, frame).await,
            None => self.symbolizer.fill_symbol(module, frame).await,
        }
    }

    async fn walk_frame(
        &self,
        module: &(dyn Module + Sync),
        walker: &mut (dyn FrameWalker + Send),
    ) -> Option<()> {
        match self.renamed(module) {
            Some(renamed) => self.symbolizer.walk_frame(&renamed, walker).await,
            None => self.symbolizer.walk_frame(module, walker).await,
        }
    }

    async fn get_file_path(
        &self,
        module: &(dyn Module + Sync),
        kind: FileKind,
    ) -> Result<PathBuf, FileError> {
        match self.renamed(module) {
            Some(renamed) => self.symbolizer.get_file_path(&renamed, kind).await,
            None => self.symbolizer.get_file_path(module, kind).await,
        }
    }
}
