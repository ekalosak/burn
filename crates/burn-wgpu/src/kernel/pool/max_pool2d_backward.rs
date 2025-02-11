use burn_tensor::ElementConversion;

use crate::{
    codegen::{
        dialect::gpu::{gpu, Elem, Item, Scope, Variable, Visibility},
        execute_dynamic, Compilation, CompilationInfo, CompilationSettings, Compiler, EagerHandle,
        InputInfo, OutputInfo, WorkgroupLaunch,
    },
    element::JitElement,
    kernel::{self, DynamicKernelSource, SourceTemplate},
    ops::numeric::empty_device,
    tensor::JitTensor,
    Runtime,
};
use std::marker::PhantomData;

#[derive(new)]
struct MaxPool2dWithIndicesBackwardEagerKernel<R: Runtime, E: JitElement> {
    kernel_size: [usize; 2],
    _runtime: PhantomData<R>,
    _elem: PhantomData<E>,
}

struct MaxPool2dBackwardComputeShader {
    indices: Variable,
    grad: Variable,
    output: Variable,
    kernel_size: [usize; 2],
}

impl MaxPool2dBackwardComputeShader {
    fn expand(self, scope: &mut Scope) {
        let grad = self.grad;
        let output = self.output;
        let indices = self.indices;
        let id = Variable::Id;

        let grad_stride_0 = scope.create_local(Elem::UInt);
        let grad_stride_1 = scope.create_local(Elem::UInt);
        let grad_stride_2 = scope.create_local(Elem::UInt);
        let grad_stride_3 = scope.create_local(Elem::UInt);

        let grad_shape_2 = scope.create_local(Elem::UInt);
        let grad_shape_3 = scope.create_local(Elem::UInt);

        let output_stride_0 = scope.create_local(Elem::UInt);
        let output_stride_1 = scope.create_local(Elem::UInt);
        let output_stride_2 = scope.create_local(Elem::UInt);
        let output_stride_3 = scope.create_local(Elem::UInt);

        let output_shape_0 = scope.create_local(Elem::UInt);
        let output_shape_1 = scope.create_local(Elem::UInt);
        let output_shape_2 = scope.create_local(Elem::UInt);
        let output_shape_3 = scope.create_local(Elem::UInt);

        gpu!(scope, grad_stride_0 = stride(grad, 0u32));
        gpu!(scope, grad_stride_1 = stride(grad, 1u32));
        gpu!(scope, grad_stride_2 = stride(grad, 2u32));
        gpu!(scope, grad_stride_3 = stride(grad, 3u32));

        gpu!(scope, grad_shape_2 = shape(grad, 2u32));
        gpu!(scope, grad_shape_3 = shape(grad, 3u32));

        gpu!(scope, output_stride_0 = stride(output, 0u32));
        gpu!(scope, output_stride_1 = stride(output, 1u32));
        gpu!(scope, output_stride_2 = stride(output, 2u32));
        gpu!(scope, output_stride_3 = stride(output, 3u32));

        gpu!(scope, output_shape_0 = shape(output, 0u32));
        gpu!(scope, output_shape_1 = shape(output, 1u32));
        gpu!(scope, output_shape_2 = shape(output, 2u32));
        gpu!(scope, output_shape_3 = shape(output, 3u32));

        let b = scope.create_local(Elem::UInt);
        let c = scope.create_local(Elem::UInt);
        let ih = scope.create_local(Elem::UInt);
        let iw = scope.create_local(Elem::UInt);

        gpu!(scope, b = id / output_stride_0);
        gpu!(scope, b = b % output_shape_0);

        gpu!(scope, c = id / output_stride_1);
        gpu!(scope, c = c % output_shape_1);

        gpu!(scope, ih = id / output_stride_2);
        gpu!(scope, ih = ih % output_shape_2);

        gpu!(scope, iw = id / output_stride_3);
        gpu!(scope, iw = iw % output_shape_3);

        let index_current = scope.create_local(Elem::UInt);
        let index_current_tmp = scope.create_local(Elem::UInt);

        gpu!(scope, index_current = ih * output_stride_2);
        gpu!(scope, index_current_tmp = iw * output_stride_3);
        gpu!(scope, index_current += index_current_tmp);

        let index_select = scope.create_local(Elem::Int);

        let index_max = scope.create_local(Elem::UInt);
        let is_max = scope.create_local(Elem::Bool);

        let index = scope.create_local(Elem::UInt);
        let index_tmp = scope.create_local(Elem::UInt);

        let grad_accumulation = scope.zero(grad.item());
        let result = scope.create_local(grad.item());

        let (oh_start, oh_end, ow_start, ow_end) = self.loop_ranges(
            scope,
            ih,
            iw,
            grad_shape_2,
            grad_shape_3,
            output_stride_2,
            output_stride_3,
        );

        gpu!(
            scope,
            range(oh_start, oh_end).for_each(|oh, scope| {
                gpu!(
                    scope,
                    range(ow_start, ow_end).for_each(|ow, scope| {
                        gpu!(scope, index = b * grad_stride_0);

                        gpu!(scope, index_tmp = c * grad_stride_1);
                        gpu!(scope, index += index_tmp);

                        gpu!(scope, index_tmp = oh * grad_stride_2);
                        gpu!(scope, index += index_tmp);

                        gpu!(scope, index_tmp = ow * grad_stride_3);
                        gpu!(scope, index += index_tmp);

                        gpu!(scope, index_select = indices[index]);
                        gpu!(scope, index_max = cast(index_select));

                        gpu!(scope, is_max = index_max == index_current);

                        gpu!(scope, if(is_max).then(|scope|{
                            gpu!(scope, result = grad[index]);
                            gpu!(scope, grad_accumulation += result);
                        }));
                    })
                );
            })
        );

        gpu!(scope, output[id] = grad_accumulation);
    }

    #[allow(clippy::too_many_arguments)]
    fn loop_ranges(
        self,
        scope: &mut Scope,
        ih: Variable,
        iw: Variable,
        grad_shape_2: Variable,
        grad_shape_3: Variable,
        output_stride_2: Variable,
        output_stride_3: Variable,
    ) -> (Variable, Variable, Variable, Variable) {
        let pool_stride_0 = Variable::GlobalScalar(0, Elem::UInt);
        let pool_stride_1 = Variable::GlobalScalar(1, Elem::UInt);
        let dilation_0 = Variable::GlobalScalar(2, Elem::UInt);
        let dilation_1 = Variable::GlobalScalar(3, Elem::UInt);
        let padding_0 = Variable::GlobalScalar(4, Elem::UInt);
        let padding_1 = Variable::GlobalScalar(5, Elem::UInt);

        let [kernel_size_0, kernel_size_1] = self.kernel_size;

        let signed_ih = scope.create_local(Elem::Int);
        let signed_iw = scope.create_local(Elem::Int);

        let signed_pool_stride_0 = scope.create_local(Elem::Int);
        let signed_pool_stride_1 = scope.create_local(Elem::Int);
        let signed_dilation_0 = scope.create_local(Elem::Int);
        let signed_dilation_1 = scope.create_local(Elem::Int);
        let signed_padding_0 = scope.create_local(Elem::Int);
        let signed_padding_1 = scope.create_local(Elem::Int);
        let signed_kernel_size_0 = scope.create_local(Elem::Int);
        let signed_kernel_size_1 = scope.create_local(Elem::Int);

        gpu!(scope, signed_pool_stride_0 = cast(pool_stride_0));
        gpu!(scope, signed_pool_stride_1 = cast(pool_stride_1));
        gpu!(scope, signed_dilation_0 = cast(dilation_0));
        gpu!(scope, signed_dilation_1 = cast(dilation_1));
        gpu!(scope, signed_padding_0 = cast(padding_0));
        gpu!(scope, signed_padding_1 = cast(padding_1));

        gpu!(scope, signed_kernel_size_0 = cast(kernel_size_0));
        gpu!(scope, signed_kernel_size_1 = cast(kernel_size_1));

        gpu!(scope, signed_ih = cast(ih));
        gpu!(scope, signed_iw = cast(iw));

        let kms_0 = scope.create_local(Elem::Int);
        let kms_1 = scope.create_local(Elem::Int);

        gpu!(scope, kms_0 = signed_dilation_0 * signed_kernel_size_0);
        gpu!(scope, kms_0 = kms_0 - signed_pool_stride_0);

        gpu!(scope, kms_1 = signed_dilation_1 * signed_kernel_size_1);
        gpu!(scope, kms_1 = kms_1 - signed_pool_stride_1);

        let oh_start_tmp = scope.create_local(Elem::Int);
        let ow_start_tmp = scope.create_local(Elem::Int);

        gpu!(scope, oh_start_tmp = signed_ih + signed_padding_0);
        gpu!(scope, oh_start_tmp = oh_start_tmp - kms_0);
        gpu!(scope, oh_start_tmp = oh_start_tmp / signed_pool_stride_0);

        gpu!(scope, ow_start_tmp = signed_iw + signed_padding_1);
        gpu!(scope, ow_start_tmp = ow_start_tmp - kms_1);
        gpu!(scope, ow_start_tmp = ow_start_tmp / signed_pool_stride_1);

        gpu!(scope, oh_start_tmp = max(oh_start_tmp, 0i32));
        gpu!(scope, ow_start_tmp = max(ow_start_tmp, 0i32));

        let oh_start = scope.create_local(Elem::UInt);
        let ow_start = scope.create_local(Elem::UInt);

        gpu!(scope, oh_start = cast(oh_start_tmp));
        gpu!(scope, ow_start = cast(ow_start_tmp));

        let oh_end_tmp = scope.create_local(Elem::Int);
        let ow_end_tmp = scope.create_local(Elem::Int);

        gpu!(scope, oh_end_tmp = max(kms_0, 0i32));
        gpu!(scope, ow_end_tmp = max(kms_1, 0i32));

        let oh_end = scope.create_local(Elem::UInt);
        let ow_end = scope.create_local(Elem::UInt);

        let oh_end_limit = scope.create_local(Elem::UInt);
        let ow_end_limit = scope.create_local(Elem::UInt);

        gpu!(scope, oh_end = cast(oh_end_tmp));
        gpu!(scope, ow_end = cast(ow_end_tmp));

        gpu!(scope, oh_end = oh_end + oh_start);
        gpu!(scope, oh_end_limit = grad_shape_2 - 1u32);

        gpu!(scope, ow_end = ow_end + ow_start);
        gpu!(scope, ow_end_limit = grad_shape_3 - 1u32);

        gpu!(scope, oh_end = min(oh_end, oh_end_limit));
        gpu!(scope, ow_end = min(ow_end, ow_end_limit));

        let index_current = scope.create_local(Elem::UInt);
        let index_current_tmp = scope.create_local(Elem::UInt);

        gpu!(scope, index_current = ih * output_stride_2);
        gpu!(scope, index_current_tmp = iw * output_stride_3);
        gpu!(scope, index_current += index_current_tmp);

        gpu!(scope, oh_end = oh_end + 1u32);
        gpu!(scope, ow_end = ow_end + 1u32);

        (oh_start, oh_end, ow_start, ow_end)
    }
}

impl<R: Runtime, E: JitElement> DynamicKernelSource
    for MaxPool2dWithIndicesBackwardEagerKernel<R, E>
{
    fn source(&self) -> kernel::SourceTemplate {
        let mut scope = Scope::root();
        let item = E::gpu_elem().into();

        let indices = Variable::GlobalInputArray(0, Item::Scalar(Elem::Int));
        let grad = Variable::GlobalInputArray(1, item);
        let output = Variable::GlobalOutputArray(0, item);

        scope.write_global_custom(output);

        MaxPool2dBackwardComputeShader {
            indices,
            grad,
            output,
            kernel_size: self.kernel_size,
        }
        .expand(&mut scope);

        let indices = InputInfo::Array {
            item: Item::Scalar(Elem::Int),
            visibility: Visibility::Read,
        };

        let grad = InputInfo::Array {
            item,
            visibility: Visibility::Read,
        };
        let scalars = InputInfo::Scalar {
            elem: Elem::UInt,
            size: 6,
        };
        let output = OutputInfo::Array { item };

        let info = CompilationInfo {
            inputs: vec![indices, grad, scalars],
            outputs: vec![output],
            scope,
        };

        let settings = CompilationSettings::default();
        let shader = Compilation::new(info).compile(settings);
        let shader = <R::Compiler as Compiler>::compile(shader);
        SourceTemplate::new(shader.to_string())
    }

    fn id(&self) -> String {
        format!(
            "{:?}k={:?}",
            core::any::TypeId::of::<Self>(),
            self.kernel_size,
        )
    }
}

pub(crate) fn max_pool2d_with_indices_backward<R: Runtime, E: JitElement, I: JitElement>(
    x: JitTensor<R, E, 4>,
    grad: JitTensor<R, E, 4>,
    indices: JitTensor<R, I, 4>,
    kernel_size: [usize; 2],
    stride: [usize; 2],
    padding: [usize; 2],
    dilation: [usize; 2],
) -> JitTensor<R, E, 4> {
    let grad = kernel::into_contiguous(grad);
    let indices = kernel::into_contiguous(indices);

    let output = empty_device(x.client.clone(), x.device.clone(), x.shape.clone());
    let kernel = MaxPool2dWithIndicesBackwardEagerKernel::new(kernel_size);

    execute_dynamic::<R, MaxPool2dWithIndicesBackwardEagerKernel<R, E>, I>(
        &[
            EagerHandle::new(&indices.handle, &indices.strides, &indices.shape.dims),
            EagerHandle::new(&grad.handle, &grad.strides, &grad.shape.dims),
        ],
        &[EagerHandle::new(
            &output.handle,
            &output.strides,
            &output.shape.dims,
        )],
        Some(&[
            (stride[0] as i32).elem(),
            (stride[1] as i32).elem(),
            (dilation[0] as i32).elem(),
            (dilation[1] as i32).elem(),
            (padding[0] as i32).elem(),
            (padding[1] as i32).elem(),
        ]),
        kernel,
        WorkgroupLaunch::Output { pos: 0 },
        x.client,
    );

    output
}

#[cfg(test)]
mod tests {
    use crate::tests::{ReferenceBackend, TestBackend};
    use burn_tensor::{module, ops::ModuleOps, Distribution, Tensor};

    #[test]
    pub fn max_pool2d_with_indices_backward_should_work_with_multiple_invocations() {
        let test_device = Default::default();
        let tensor =
            Tensor::<TestBackend, 4>::random([32, 32, 32, 32], Distribution::Default, &test_device);
        let grad_output =
            Tensor::<TestBackend, 4>::random([32, 32, 16, 16], Distribution::Default, &test_device);
        let ref_device = Default::default();
        let tensor_ref = Tensor::<ReferenceBackend, 4>::from_data(tensor.to_data(), &ref_device);
        let grad_output_ref =
            Tensor::<ReferenceBackend, 4>::from_data(grad_output.to_data(), &ref_device);
        let kernel_size = [3, 3];
        let stride = [2, 2];
        let padding = [1, 1];
        let dilation = [1, 1];

        let (_, indices) =
            module::max_pool2d_with_indices(tensor.clone(), kernel_size, stride, padding, dilation);
        let (_, indices_ref) = module::max_pool2d_with_indices(
            tensor_ref.clone(),
            kernel_size,
            stride,
            padding,
            dilation,
        );
        let grad = TestBackend::max_pool2d_with_indices_backward(
            tensor.into_primitive(),
            kernel_size,
            stride,
            padding,
            dilation,
            grad_output.into_primitive(),
            indices.into_primitive(),
        )
        .x_grad;
        let grad_ref = ReferenceBackend::max_pool2d_with_indices_backward(
            tensor_ref.into_primitive(),
            kernel_size,
            stride,
            padding,
            dilation,
            grad_output_ref.into_primitive(),
            indices_ref.into_primitive(),
        )
        .x_grad;

        Tensor::<TestBackend, 4>::from_primitive(grad)
            .into_data()
            .assert_approx_eq(
                &Tensor::<ReferenceBackend, 4>::from_primitive(grad_ref).into_data(),
                3,
            );
    }
}
