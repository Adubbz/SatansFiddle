use crate::config;

use digest_io::IoWrapper;
use hex_literal::hex;
use lldb::*;
use sha2::*;
use std::*;

#[derive(Debug)]
pub struct Compiler {
    version: &'static str,
    sha256: &'static [u8; 32],

    // Function addresses
    pub func_compile_translation_unit_addr: u64,

    // Variables
    var_gpr_mask_addr: u64,
    var_fpr_mask_addr: u64,
    var_tu_name_addr: u64,
}

impl Compiler {
    fn read_translation_unit_name(&self, process: &SBProcess) -> Result<String, SBError> {
        let mut len_buffer = [0u8; 1];
        process.read_memory(self.var_tu_name_addr, &mut len_buffer)?;
        
        let mut name_buffer = vec![0u8; u8::from_le_bytes(len_buffer) as usize];
        process.read_memory(self.var_tu_name_addr + 1, &mut name_buffer)?;
        
        Ok(String::from_utf8_lossy(&name_buffer).to_string())
    }

    fn set_gpr_helper_mask(&self, process: &SBProcess, mask: u32) {
        process.write_memory(self.var_gpr_mask_addr, &mask.to_le_bytes());
    }

    fn set_fpr_helper_mask(&self, process: &SBProcess, mask: u32) {
        process.write_memory(self.var_fpr_mask_addr, &mask.to_le_bytes());
    }
}

static COMPILERS: &[Compiler] = &[
    Compiler {
        version: "2.3.3",
        sha256: &hex!("6375fd27814a1cb7eddbc229ecbd15b57d83562f95be23804f0a06ad4814e2df"),
        func_compile_translation_unit_addr: 0x00432040,
        var_gpr_mask_addr: 0x0051CE00,
        var_fpr_mask_addr: 0x0051CE04,
        var_tu_name_addr:  0x00557B28,
    },
];

static CURRENT_COMPILER: sync::OnceLock<&'static Compiler> = sync::OnceLock::new();

pub fn compile_translation_unit(process: &SBProcess, _thread: &SBThread, _location: &SBBreakpointLocation,) -> bool {
    let compiler=  CURRENT_COMPILER.get().expect("Compiler not initialized");
    let name = compiler.read_translation_unit_name(process).expect("Failed to read translation unit name");

    // Read the configured masks and write them to memory for the current translation unit
    let gpr_helper_mask = config::gpr_helper_mask(&name);
    let fpr_helper_mask = config::fpr_helper_mask(&name);

    compiler.set_gpr_helper_mask(process, gpr_helper_mask);
    compiler.set_fpr_helper_mask(process, fpr_helper_mask);

    false
}

pub fn load_compiler(path: &path::Path) {
    let mut file = fs::File::open(path).expect("Failed to open compiler file");
    let mut hasher = IoWrapper(Sha256::new());

    // Calculate the hash for the file
    io::copy(&mut file, &mut hasher).expect("Failed to copy file to hasher");
    let result = hasher.0.finalize();
    
    // Find a compiler with a matching hash
    for compiler in COMPILERS {
        if *compiler.sha256 == *result {
            CURRENT_COMPILER.set(compiler).expect("Failed to set current compiler");
            return
        }
    }

    // Find a compiler with a matching version
    if let Some(version) = config::compiler_version() {
        for compiler in COMPILERS {
            if compiler.version == version {
                CURRENT_COMPILER.set(compiler).expect("Failed to set current compiler");
                return
            }
        }
    }

    panic!("No matching compiler found")
}

pub fn get_compiler() -> &'static Compiler {
    CURRENT_COMPILER.get().expect("Compiler not initialized")
}
