//! Device and Host memory handlers

use super::*;
use crate::*;
use cuda::*;
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use cuda::CUmemAttach_flags_enum as AttachFlag;

/// Memory allocated on the device.
pub struct DeviceMemory<T> {
    ptr: CUdeviceptr,
    size: usize,
    context: Arc<Context>,
    phantom: PhantomData<T>,
}

impl<T> Drop for DeviceMemory<T> {
    fn drop(&mut self) {
        if let Err(e) = unsafe { contexted_call!(self, cuMemFree_v2, self.ptr) } {
            log::error!("Failed to free device memory: {:?}", e);
        }
    }
}

impl<T> Deref for DeviceMemory<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr as _, self.size) }
    }
}

impl<T> DerefMut for DeviceMemory<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr as _, self.size) }
    }
}

impl<T: Scalar> Memory for DeviceMemory<T> {
    type Elem = T;
    fn head_addr(&self) -> *const T {
        self.ptr as _
    }

    fn head_addr_mut(&mut self) -> *mut T {
        self.ptr as _
    }

    fn num_elem(&self) -> usize {
        self.size
    }

    fn memory_type(&self) -> MemoryType {
        MemoryType::Device
    }

    fn try_as_slice(&self) -> Option<&[T]> {
        Some(self.as_slice())
    }

    fn try_as_mut_slice(&mut self) -> Option<&mut [T]> {
        Some(self.as_mut_slice())
    }

    fn try_get_context(&self) -> Option<Arc<Context>> {
        Some(self.get_context())
    }
}

/// Safety
/// ------
/// - This works only when `mem` is device memory
pub(super) unsafe fn memset_device<T, Mem>(mem: &mut Mem, value: T) -> error::Result<()>
where
    T: Scalar,
    Mem: Continuous<Elem = T> + ?Sized,
{
    assert_eq!(mem.memory_type(), MemoryType::Device);
    match T::size_of() {
        bytes @ 1 | bytes @ 2 | bytes @ 4 => {
            let ptr = mem.head_addr_mut() as _;
            let size = mem.num_elem();
            let ctx = mem.try_get_context().unwrap();
            match bytes {
                1 => {
                    let value = value.to_le_u8().unwrap();
                    contexted_call!(&ctx, cuMemsetD8_v2, ptr, value, size)?;
                }
                2 => {
                    let value = value.to_le_u16().unwrap();
                    contexted_call!(&ctx, cuMemsetD16_v2, ptr, value, size)?;
                }
                4 => {
                    let value = value.to_le_u32().unwrap();
                    contexted_call!(&ctx, cuMemsetD32_v2, ptr, value, size)?;
                }
                _ => unreachable!(),
            }
        }
        _ => mem.as_mut_slice().iter_mut().for_each(|v| *v = value),
    }
    Ok(())
}

/// Safety
/// ------
/// - This works only when `dest` is device memory
pub(super) unsafe fn copy_to_device<T: Scalar, Dest, Src>(dest: &mut Dest, src: &Src)
where
    Dest: Memory<Elem = T> + ?Sized,
    Src: Memory<Elem = T> + ?Sized,
{
    assert_eq!(dest.memory_type(), MemoryType::Device);
    assert_ne!(dest.head_addr(), src.head_addr());
    assert_eq!(dest.num_elem(), src.num_elem());

    let dest_ptr = dest.head_addr_mut();
    let src_ptr = src.head_addr();

    // context guard
    let _g = match (dest.try_get_context(), src.try_get_context()) {
        (Some(d_ctx), Some(s_ctx)) => {
            assert_eq!(d_ctx, s_ctx);
            Some(d_ctx.guard_context())
        }
        (Some(ctx), None) => Some(ctx.guard_context()),
        (None, Some(ctx)) => Some(ctx.guard_context()),
        (None, None) => None,
    };

    match src.memory_type() {
        // From host
        MemoryType::Host | MemoryType::Registered | MemoryType::PageLocked => {
            ffi_call!(
                cuMemcpyHtoD_v2,
                dest_ptr as _,
                src_ptr as _,
                dest.num_elem() * std::mem::size_of::<T>()
            )
            .expect("memcpy from Host to Device failed");
        }
        // From device
        MemoryType::Device => {
            ffi_call!(
                cuMemcpyDtoD_v2,
                dest_ptr as _,
                src_ptr as _,
                dest.num_elem() * std::mem::size_of::<T>()
            )
            .expect("memcpy from Device to Device failed");
        }
        // From array
        MemoryType::Array => unimplemented!("Array memory is not supported yet"),
    }
}

impl<T: Scalar> Memcpy<Self> for DeviceMemory<T> {
    fn copy_from(&mut self, src: &Self) {
        unsafe { copy_to_device(self, src) }
    }
}

impl<T: Scalar> Memcpy<PageLockedMemory<T>> for DeviceMemory<T> {
    fn copy_from(&mut self, src: &PageLockedMemory<T>) {
        unsafe { copy_to_device(self, src) }
    }
}

impl<T: Scalar> Memcpy<[T]> for DeviceMemory<T> {
    fn copy_from(&mut self, src: &[T]) {
        unsafe { copy_to_device(self, src) }
    }
}

impl<T: Scalar> Memset for DeviceMemory<T> {
    fn set(&mut self, value: Self::Elem) {
        unsafe { memset_device(self, value).expect("memset failed") };
    }
}

impl<T: Scalar> Continuous for DeviceMemory<T> {
    fn as_slice(&self) -> &[T] {
        self
    }
    fn as_mut_slice(&mut self) -> &mut [T] {
        self
    }
}

impl<T: Scalar> Managed for DeviceMemory<T> {}

impl<T> Contexted for DeviceMemory<T> {
    fn get_context(&self) -> Arc<Context> {
        self.context.clone()
    }
}

impl<T: Scalar> Allocatable for DeviceMemory<T> {
    type Shape = usize;
    unsafe fn uninitialized(context: Arc<Context>, size: usize) -> Self {
        assert!(size > 0, "Zero-sized malloc is forbidden");
        let ptr = contexted_new!(
            &context,
            cuMemAllocManaged,
            size * std::mem::size_of::<T>(),
            AttachFlag::CU_MEM_ATTACH_GLOBAL as u32
        )
        .expect("Cannot allocate device memory");
        DeviceMemory {
            ptr,
            size,
            context,
            phantom: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::*;

    #[test]
    fn device() -> Result<()> {
        let device = Device::nth(0)?;
        let ctx = device.create_context();
        let mut mem = DeviceMemory::<i32>::zeros(ctx, 12);
        assert_eq!(mem.num_elem(), 12);
        let sl = mem.as_mut_slice();
        sl[0] = 3;
        Ok(())
    }

    #[should_panic(expected = "Zero-sized malloc is forbidden")]
    #[test]
    fn device_new_zero() {
        let device = Device::nth(0).unwrap();
        let ctx = device.create_context();
        let _a = DeviceMemory::<i32>::zeros(ctx, 0);
    }
}