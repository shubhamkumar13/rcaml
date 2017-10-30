extern crate llvm_sys as llvm;

use self::llvm::core::*;
use self::llvm::prelude::*;

extern crate libc;

use std::ffi::CString;
use std::ptr;
use std::boxed::Box;
use std::collections::{HashMap, hash_map, VecDeque};

use node::{NodeKind, FuncDef, BinOps};
use node;

use closure::{Closure, Prog};

use typing::{Type, TypeScheme};
use typing;

use parser::EXTENV;

#[derive(Eq, PartialEq, Hash)]
pub struct ExtFunc {
    llvm_val: LLVMValueRef,
    ty: Type,
}

pub struct CodeGen<'a> {
    context: LLVMContextRef,
    module: LLVMModuleRef,
    builder: LLVMBuilderRef,
    exec_engine: llvm::execution_engine::LLVMExecutionEngineRef,
    tyenv: &'a mut HashMap<usize, Type>,
    ext_funcmap: HashMap<String, ExtFunc>,
    global_varmap: HashMap<String, (Type, LLVMTypeRef, LLVMValueRef)>,
}

pub enum CodeGenError {
    Something,
}

type CodeGenResult<T> = Result<T, CodeGenError>;

#[no_mangle]
pub extern "C" fn print_int(i: i32) {
    print!("{}", i);
}
#[no_mangle]
pub extern "C" fn print_float(f: f64) {
    print!("{}", f);
}
#[no_mangle]
pub extern "C" fn print_newline(_: i32) {
    print!("\n");
}

impl<'a> CodeGen<'a> {
    pub unsafe fn new(tyenv: &'a mut HashMap<usize, Type>) -> Self {
        llvm::execution_engine::LLVMLinkInMCJIT();
        llvm::target::LLVM_InitializeAllTargetMCs();
        llvm::target::LLVM_InitializeNativeTarget();
        llvm::target::LLVM_InitializeNativeAsmPrinter();
        llvm::target::LLVM_InitializeNativeAsmParser();

        let context = LLVMContextCreate();

        let c_mod_name = CString::new("rcaml").unwrap();
        let module = LLVMModuleCreateWithNameInContext(c_mod_name.as_ptr(), context);

        let mut ee = 0 as llvm::execution_engine::LLVMExecutionEngineRef;
        let mut error = 0 as *mut i8;
        if llvm::execution_engine::LLVMCreateExecutionEngineForModule(
            &mut ee,
            module,
            &mut error,
        ) != 0
        {
            panic!("err");
        }


        let mut ext_funcmap = HashMap::new();
        // initialize standard functions
        {
            let f_print_int_ty = LLVMFunctionType(
                LLVMVoidType(),
                vec![LLVMInt32Type()].as_mut_slice().as_mut_ptr(),
                1,
                0,
            );
            let f_print_int = LLVMAddFunction(
                module,
                CString::new("print_int").unwrap().as_ptr(),
                f_print_int_ty,
            );
            ext_funcmap.insert(
                "print_int".to_string(),
                ExtFunc {
                    ty: Type::Func(vec![Type::Int], Box::new(Type::Unit)),
                    llvm_val: f_print_int,
                },
            );
            llvm::execution_engine::LLVMAddGlobalMapping(
                ee,
                f_print_int,
                print_int as *mut libc::c_void,
            );

            let f_print_float_ty = LLVMFunctionType(
                LLVMVoidType(),
                vec![LLVMDoubleType()].as_mut_slice().as_mut_ptr(),
                1,
                0,
            );
            let f_print_float = LLVMAddFunction(
                module,
                CString::new("print_float").unwrap().as_ptr(),
                f_print_float_ty,
            );
            ext_funcmap.insert(
                "print_float".to_string(),
                ExtFunc {
                    ty: Type::Func(vec![Type::Float], Box::new(Type::Unit)),
                    llvm_val: f_print_float,
                },
            );
            llvm::execution_engine::LLVMAddGlobalMapping(
                ee,
                f_print_float,
                print_float as *mut libc::c_void,
            );

            let f_print_newline_ty =
                LLVMFunctionType(LLVMVoidType(), vec![].as_mut_slice().as_mut_ptr(), 0, 0);
            let f_print_newline = LLVMAddFunction(
                module,
                CString::new("print_newline").unwrap().as_ptr(),
                f_print_newline_ty,
            );
            ext_funcmap.insert(
                "print_newline".to_string(),
                ExtFunc {
                    ty: Type::Func(vec![Type::Unit], Box::new(Type::Unit)),
                    llvm_val: f_print_newline,
                },
            );
            llvm::execution_engine::LLVMAddGlobalMapping(
                ee,
                f_print_newline,
                print_newline as *mut libc::c_void,
            );
        }

        CodeGen {
            context: context,
            module: module,
            builder: LLVMCreateBuilderInContext(context),
            exec_engine: ee,
            tyenv: tyenv,
            ext_funcmap: ext_funcmap,
            global_varmap: HashMap::new(),
        }
    }

    pub unsafe fn gen(&mut self, progs: Vec<Prog>) -> CodeGenResult<LLVMValueRef> {
        let main_ty = LLVMFunctionType(LLVMInt32Type(), vec![].as_mut_slice().as_mut_ptr(), 0, 0);
        let main = LLVMAddFunction(self.module, CString::new("main").unwrap().as_ptr(), main_ty);
        let bb_entry = LLVMAppendBasicBlock(main, CString::new("entry").unwrap().as_ptr());
        LLVMPositionBuilderAtEnd(self.builder, bb_entry);

        // let mut funcs = Vec::new();

        for Prog(func, expr) in progs {
            match &expr {
                // &NodeKind::LetFuncDef(ref funcdef, ref expr) => funcs.push(node.clone()),
                _ => {
                    try!(self.gen_expr(&HashMap::new(), &expr));
                    ()
                }
            }
        }

        LLVMBuildRet(self.builder, try!(self.gen_int(0)));

        LLVMDumpModule(self.module);

        println!("*** running main ***");
        llvm::execution_engine::LLVMRunFunction(
            self.exec_engine,
            main,
            0,
            vec![].as_mut_slice().as_mut_ptr(),
        );
        println!("*** end of main ***");

        Ok(ptr::null_mut())
    }

    unsafe fn gen_expr(
        &mut self,
        env: &HashMap<String, LLVMValueRef>,
        closure: &Closure,
    ) -> CodeGenResult<LLVMValueRef> {
        match closure {
            &Closure::LetExpr((ref name, ref ty), ref expr, ref body) => {
                self.gen_letexpr(env, name, ty, expr, body)
            }
            // LetExpr((String, typing::Type), Box<NodeKind>, Box<NodeKind>), // (name, ty), bound expr, body
            // LetFuncExpr(FuncDef, Box<NodeKind>, Box<NodeKind>), // (name, ty), bound expr, body
            // LetDef((String, typing::Type), Box<NodeKind>), // name, bound expr
            // LetFuncDef(FuncDef, Box<NodeKind>), // name, bound expr

            // TODO: below
            // &NodeKind::LetDef((ref name, ref ty), ref expr) => self.gen_letdef(name, ty, &*expr),
            // &NodeKind::LetFuncDef(ref funcdef, ref expr) => self.gen_letfuncdef(&*funcdef, &*expr),

            // Call(Box<NodeKind>, Vec<NodeKind>),
            &Closure::AppCls(ref callee, ref args) => self.gen_call(env, &*callee, &*args),
            // &NodeKind::Call(ref callee, ref args) => self.gen_call(&*callee, &*args),

            // IntBinaryOp(BinOps, Box<NodeKind>, Box<NodeKind>),
            // &NodeKind::IntBinaryOp(ref op, ref lhs, ref rhs) => {
            //     self.gen_int_binop(op, &*lhs, &*rhs)
            // }
            &Closure::IntBinaryOp(ref op, ref lhs, ref rhs) => {
                self.gen_int_binop(env, &*op, &*lhs, &*rhs)
            }
            &Closure::FloatBinaryOp(ref op, ref lhs, ref rhs) => {
                self.gen_float_binop(env, &*op, &*lhs, &*rhs)
            }
            // &NodeKind:: FloatUnaryOp(UnaryOps, Box<NodeKind>)
            &Closure::Var(ref name) => self.gen_var_load(env, name),
            &Closure::Int(ref i) => self.gen_int(*i),
            &Closure::Float(ref f) => self.gen_float(f.into_inner()),
            &Closure::Unit => self.gen_int(0), // tmp
            _ => panic!("not implemented"),
        }
    }

    pub unsafe fn gen_letexpr(
        &mut self,
        env: &HashMap<String, LLVMValueRef>,
        name: &String,
        ty: &Type,
        expr: &Closure,
        body: &Closure,
    ) -> CodeGenResult<LLVMValueRef> {
        let llvm_expr_val = try!(self.gen_expr(env, expr));
        let mut newenv = env.clone();
        LLVMDumpValue(llvm_expr_val);
        newenv.insert(name.to_owned(), llvm_expr_val);
        self.gen_expr(&newenv, body)
    }

    // pub unsafe fn gen_letdef(
    //     &mut self,
    //     name: &String,
    //     ty: &Type,
    //     expr: &NodeKind,
    // ) -> CodeGenResult<LLVMValueRef> {
    //     let llvm_ty = ty.to_llvmty();
    //     let llvm_val = LLVMAddGlobal(
    //         self.module,
    //         llvm_ty,
    //         CString::new(name.as_str()).unwrap().as_ptr(),
    //     );
    //     LLVMSetInitializer(llvm_val, LLVMConstNull(llvm_ty));
    //
    //     self.global_varmap.insert(name.clone(), (
    //         ty.clone(),
    //         llvm_ty,
    //         llvm_val,
    //     ));
    //
    //     let llvm_expr = try!(self.gen_expr(expr));
    //
    //     LLVMBuildStore(self.builder, llvm_expr, llvm_val);
    //
    //     Ok(llvm_val)
    // }
    //
    // pub unsafe fn gen_letfuncdef(
    //     &mut self,
    //     funcdef: &FuncDef,
    //     expr: &NodeKind,
    // ) -> CodeGenResult<LLVMValueRef> {
    //     let (ref func_name, ref func_ret_ty) = funcdef.name;
    //     Ok(ptr::null_mut())
    // }

    unsafe fn gen_call(
        &mut self,
        env: &HashMap<String, LLVMValueRef>,
        callee: &Closure,
        args: &Vec<Closure>,
    ) -> CodeGenResult<LLVMValueRef> {
        let name = if let &Closure::Var(ref name) = callee {
            name
        } else {
            panic!("not supported")
        };

        let (func_param_tys, func_ret_ty) = if let Some(TypeScheme {
                                                            tyvars: _,
                                                            body: func_ty,
                                                        }) =
            EXTENV.lock().unwrap().get(name).cloned()
        {
            if let Type::Func(param_tys, ret_ty) = func_ty {
                (param_tys, ret_ty)
            } else {
                panic!("not func");
            }
        } else {
            panic!("not found func");
        };

        let mut args_val = Vec::new();
        for arg in args {
            let llvm_arg = try!(self.gen_expr(env, &arg));
            args_val.push(llvm_arg);
        }

        if let Some(func) = self.ext_funcmap.get(name) {
            Ok(LLVMBuildCall(
                self.builder,
                func.llvm_val,
                args_val.as_mut_slice().as_mut_ptr(),
                args_val.len() as u32,
                CString::new("").unwrap().as_ptr(),
            ))
        } else {
            Ok(ptr::null_mut())
        }
    }

    unsafe fn gen_int_binop(
        &mut self,
        env: &HashMap<String, LLVMValueRef>,
        op: &BinOps,
        lhs: &Closure,
        rhs: &Closure,
    ) -> CodeGenResult<LLVMValueRef> {
        let inst_name = |s: &str| CString::new(s).unwrap().as_ptr();
        let lhs_val = try!(self.gen_expr(env, lhs));
        let rhs_val = try!(self.gen_expr(env, rhs));
        match op {
            &BinOps::IAdd => {
                Ok(LLVMBuildAdd(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("add"),
                ))
            }
            &BinOps::ISub => {
                Ok(LLVMBuildSub(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("sub"),
                ))
            }
            &BinOps::IMul => {
                Ok(LLVMBuildMul(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("mul"),
                ))
            }
            &BinOps::IDiv => {
                Ok(LLVMBuildSDiv(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("div"),
                ))
            }
            &BinOps::IMod => {
                Ok(LLVMBuildSRem(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("rem"),
                ))
            }
            _ => panic!("not implemented"),
        }
    }

    unsafe fn gen_float_binop(
        &mut self,
        env: &HashMap<String, LLVMValueRef>,
        op: &BinOps,
        lhs: &Closure,
        rhs: &Closure,
    ) -> CodeGenResult<LLVMValueRef> {
        let inst_name = |s: &str| CString::new(s).unwrap().as_ptr();
        let lhs_val = try!(self.gen_expr(env, lhs));
        let rhs_val = try!(self.gen_expr(env, rhs));
        match op {
            &BinOps::FAdd => {
                Ok(LLVMBuildFAdd(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("fadd"),
                ))
            }
            &BinOps::FSub => {
                Ok(LLVMBuildFSub(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("fsub"),
                ))
            }
            &BinOps::FMul => {
                Ok(LLVMBuildFMul(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("fmul"),
                ))
            }
            &BinOps::FDiv => {
                Ok(LLVMBuildFDiv(
                    self.builder,
                    lhs_val,
                    rhs_val,
                    inst_name("fdiv"),
                ))
            }
            _ => panic!("not implemented"),
        }
    }

    unsafe fn lookup_var(
        &mut self,
        env: &HashMap<String, LLVMValueRef>,
        name: &String,
    ) -> CodeGenResult<LLVMValueRef> {
        if let Some(val) = env.get(name.as_str()) {
            Ok(*val)
        } else if let Some(&(ref _ty, _llvmty, val)) = self.global_varmap.get(name.as_str()) {
            Ok(val)
        } else {
            panic!(format!("not found variable '{}'", name))
        }
    }

    unsafe fn gen_var_load(
        &mut self,
        env: &HashMap<String, LLVMValueRef>,
        name: &String,
    ) -> CodeGenResult<LLVMValueRef> {
        let val = try!(self.lookup_var(env, name));
        // TODO
        Ok(val)
        // Ok(LLVMBuildLoad(
        //     self.builder,
        //     val,
        //     CString::new("load").unwrap().as_ptr(),
        // ))
    }

    pub unsafe fn gen_int(&mut self, i: i32) -> CodeGenResult<LLVMValueRef> {
        Ok(LLVMConstInt(LLVMInt32Type(), i as u64, 0))
    }

    pub unsafe fn gen_float(&mut self, f: f64) -> CodeGenResult<LLVMValueRef> {
        Ok(LLVMConstReal(LLVMDoubleType(), f))
    }
}

impl Type {
    pub unsafe fn to_llvmty(&self) -> LLVMTypeRef {
        match self {
            &Type::Unit => LLVMInt32Type(),
            &Type::Char => LLVMInt8Type(),
            &Type::Int => LLVMInt32Type(),
            &Type::Float => LLVMDoubleType(),
            _ => panic!(),
        }
    }
}
