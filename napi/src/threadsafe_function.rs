use crate::{check_status, ptr, sys, Env, Function, Result, Value};
use std::os::raw::{c_char, c_void};

use sys::napi_threadsafe_function_call_mode;
use sys::napi_threadsafe_function_release_mode;

pub trait ToJs: Copy + Clone {
  type Output;
  type JsValue;

  fn resolve(
    &self,
    env: &mut Env,
    output: &mut Self::Output,
  ) -> Result<(u64, Value<Self::JsValue>)>;
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadsafeFunction<T: ToJs> {
  raw_value: sys::napi_threadsafe_function,
  to_js: T,
}

unsafe impl<T: ToJs> Send for ThreadsafeFunction<T> {}
unsafe impl<T: ToJs> Sync for ThreadsafeFunction<T> {}

impl<T: ToJs> ThreadsafeFunction<T> {
  pub fn create(env: Env, func: Value<Function>, to_js: T, max_queue_size: u64) -> Result<Self> {
    let mut async_resource_name = ptr::null_mut();
    let s = "napi_rs_threadsafe_function";
    let status = unsafe {
      sys::napi_create_string_utf8(
        env.0,
        s.as_ptr() as *const c_char,
        s.len() as u64,
        &mut async_resource_name,
      )
    };
    check_status(status)?;

    let initial_thread_count: u64 = 1;
    let mut result = ptr::null_mut();
    let tsfn = ThreadsafeFunction {
      to_js,
      raw_value: result,
    };

    let ptr = Box::into_raw(Box::from(tsfn)) as *mut _ as *mut c_void;

    let status = unsafe {
      sys::napi_create_threadsafe_function(
        env.0,
        func.raw_value,
        ptr::null_mut(),
        async_resource_name,
        max_queue_size,
        initial_thread_count,
        ptr,
        Some(thread_finalize_cb::<T>),
        ptr,
        Some(call_js_cb::<T>),
        &mut result,
      )
    };
    check_status(status)?;

    Ok(ThreadsafeFunction {
      to_js,
      raw_value: result,
    })
  }

  pub fn call(
    &self,
    value: Result<T::Output>,
    mode: napi_threadsafe_function_call_mode,
  ) -> Result<()> {
    check_status(unsafe {
      sys::napi_call_threadsafe_function(
        self.raw_value,
        Box::into_raw(Box::from(value)) as *mut _ as *mut c_void,
        mode,
      )
    })
  }

  pub fn acquire(&self) -> Result<()> {
    check_status(unsafe { sys::napi_acquire_threadsafe_function(self.raw_value) })
  }

  pub fn release(&self, mode: napi_threadsafe_function_release_mode) -> Result<()> {
    check_status(unsafe { sys::napi_release_threadsafe_function(self.raw_value, mode) })
  }

  // "ref" is a keyword so that we use "refer"
  pub fn refer(&self, env: Env) -> Result<()> {
    check_status(unsafe { sys::napi_ref_threadsafe_function(env.0, self.raw_value) })
  }

  pub fn unref(&self, env: Env) -> Result<()> {
    check_status(unsafe { sys::napi_unref_threadsafe_function(env.0, self.raw_value) })
  }
}

unsafe extern "C" fn thread_finalize_cb<T: ToJs>(
  _raw_env: sys::napi_env,
  finalize_data: *mut c_void,
  _finalize_hint: *mut c_void,
) {
  // cleanup
  Box::from_raw(finalize_data as *mut ThreadsafeFunction<T>);
}

unsafe extern "C" fn call_js_cb<T: ToJs>(
  raw_env: sys::napi_env,
  js_callback: sys::napi_value,
  context: *mut c_void,
  data: *mut c_void,
) {
  let mut env = Env::from_raw(raw_env);
  let mut recv = ptr::null_mut();
  sys::napi_get_undefined(raw_env, &mut recv);

  let tsfn = Box::leak(Box::from_raw(context as *mut ThreadsafeFunction<T>));
  let val = Box::from_raw(data as *mut Result<T::Output>);

  let ret = val.and_then(move |mut v| tsfn.to_js.resolve(&mut env, &mut v));

  let status;

  // Follow the convention of Node.js async callback.
  if ret.is_ok() {
    let (argv, js_value) = ret.unwrap();
    let js_null = env.get_null().unwrap();
    let values = [js_null.raw_value, js_value.raw_value];
    status = sys::napi_call_function(
      raw_env,
      recv,
      js_callback,
      argv + 1,
      values.as_ptr(),
      ptr::null_mut(),
    );
  } else {
    let mut err = env.create_error(ret.err().unwrap()).unwrap();
    status = sys::napi_call_function(
      raw_env,
      recv,
      js_callback,
      1,
      &mut err.raw_value,
      ptr::null_mut(),
    );
  }

  debug_assert!(status == sys::napi_status::napi_ok, "CallJsCB failed");
}
