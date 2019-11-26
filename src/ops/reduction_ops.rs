use crate::ndarray_ext;
use crate::Context;
use crate::ndarray_ext::{NdArray, NdArrayView};
use crate::op;
use crate::ops;
use crate::tensor::Tensor;
use crate::Float;
use ndarray;
use std::f32;
use std::mem;

pub struct ReduceMin {
    pub keep_dims: bool,
    pub sparse_axes: bool,
}

pub struct ReduceMax {
    pub keep_dims: bool,
    pub sparse_axes: bool,
}

pub struct ReduceProd {
    pub keep_dims: bool,
    pub sparse_axes: bool,
}

pub struct ReduceSumToScalar;

pub struct ReduceSum {
    pub keep_dims: bool,
    pub sparse_axes: bool,
}

pub struct ReduceMean {
    pub keep_dims: bool,
    pub sparse_axes: bool,
}

pub struct ArgMax {
    pub axis: isize,
    pub keep_dim: bool,
}

pub struct ReduceGradCommon {
    pub should_make_broadcast_dims: bool,
    pub sparse_axes: bool,
}

macro_rules! impl_reduce_forward {
    ($forward_name:ident, $reduce_fn_name:ident, $reduce_default:ident) => {
        fn $forward_name<'v, T: Float>(
            x: &NdArrayView<'v, T>,
            mut axes: Vec<usize>,
            keep_dims: bool,
        ) -> crate::ArrRepr<'v, T> {
            let x_shape = x.shape();

            if ndarray_ext::is_scalar_shape(x_shape) {
                // case of 0 rank
                crate::ArrRepr::View(x.clone())
            } else {
                // reduction axes are empty => do nothing
                if axes.is_empty() {
                    return crate::ArrRepr::View(x.clone());
                }

                // -- main logic --
                let mut folded: Option<NdArray<T>> = None;
                axes.sort();

                for axis in axes.into_iter().rev() {
                    let func = T::$reduce_fn_name;

                    let ret = match folded {
                        Some(ref a) => {
                            a.fold_axis(ndarray::Axis(axis), T::$reduce_default(), move |&l, &r| {
                                func(l, r)
                            })
                        }
                        None => {
                            x.fold_axis(ndarray::Axis(axis), T::$reduce_default(), move |&l, &r| {
                                func(l, r)
                            })
                        }
                    };

                    if keep_dims {
                        mem::swap(&mut folded, &mut Some(ndarray_ext::expand_dims(ret, axis)));
                    } else {
                        mem::swap(&mut folded, &mut Some(ret));
                    }
                }

                crate::ArrRepr::Owned(folded.unwrap_or_else(|| x.to_owned()))
            }
        }
    };
}

impl_reduce_forward!(compute_reduce_sum, add, zero);
impl_reduce_forward!(compute_reduce_min, min, max_value);
impl_reduce_forward!(compute_reduce_max, max, min_value);
impl_reduce_forward!(compute_reduce_prod, mul, one);

#[inline]
fn preprocess_axes<T: Float>(
    x: &NdArrayView<T>,
    axes: &NdArrayView<T>,
    sparse_axes: bool,
) -> Vec<usize> {
    if sparse_axes {
        ndarray_ext::sparse_to_dense(axes)
    } else {
        ndarray_ext::normalize_negative_axes(axes, x.ndim())
    }
}

impl<'a, T: Float> op::Op<'a, T> for ReduceSumToScalar {
    fn name(&self) -> &str {
        "ReduceSumToScalar"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let x = &ctx.input(0);
        ctx.push_output(Ok(crate::ArrRepr::Owned(
            ndarray::arr0(x.sum()).into_dyn(),
        )));
    }

    fn grad(&self, gy: &'a Tensor<'a, T>, inputs: &[&'a Tensor<'a, T>], _: &'a Tensor<'a, T>, c: &mut Context<'a, T>) -> Vec<Option<&'a Tensor<'a, T>>> {
        let gx = Tensor::builder()
            .set_inputs(&[gy, c.shape(inputs[0])])
            .build(c, ReduceSumToScalarGrad);
        vec![Some(gx)]
    }
}

struct ReduceSumToScalarGrad;

impl<'a, T: Float> op::Op<'a, T> for ReduceSumToScalarGrad {
    fn name(&self) -> &str {
        "ReduceSumToScalarGrad"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let shape = ndarray_ext::as_shape(&ctx.input(1));
        let ret = unsafe {
            let x = *ctx.input(0).as_ptr();
            ndarray::ArrayD::<T>::from_elem(ndarray::IxDyn(shape.as_slice()), x)
        };
        ctx.push_output(Ok(crate::ArrRepr::Owned(ret)));
    }

    fn grad(&self, gy: &'a Tensor<'a, T>, _: &[&'a Tensor<'a, T>], _: &'a Tensor<'a, T>, c: &mut Context<'a, T>) -> Vec<Option<&'a Tensor<'a, T>>> {
        let gx = Tensor::builder().set_input(gy).build(c, ReduceSumToScalar);
        vec![Some(gx), None]
    }
}

impl<'a, T: Float> op::Op<'a, T> for ReduceSum {
    fn name(&self) -> &str {
        "ReduceSum"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let x = &ctx.input(0);
        let axes = preprocess_axes(x, &ctx.input(1), self.sparse_axes);
        ctx.push_output(Ok(compute_reduce_sum(x, axes, self.keep_dims)))
    }

    fn grad(&self, gy: &'a Tensor<'a, T>, inputs: &[&'a Tensor<'a, T>], _: &'a Tensor<'a, T>, c: &mut Context<'a, T>) -> Vec<Option<&'a Tensor<'a, T>>> {
        let grad_op = ReduceGradCommon {
            should_make_broadcast_dims: !self.keep_dims,
            sparse_axes: self.sparse_axes,
        };
        let gx = Tensor::builder()
            .set_inputs(&[gy, c.shape(inputs[0]), inputs[1]])
            .build(c, grad_op);
        vec![Some(gx), None]
    }
}

impl<'a, T: Float> op::Op<'a, T> for ReduceMean {
    fn name(&self) -> &str {
        "ReduceMean"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let x = &ctx.input(0);
        let axes = preprocess_axes(x, &ctx.input(1), self.sparse_axes);
        let x_shape = x.shape();
        if axes.is_empty() {
            return ctx.push_output(Ok(crate::ArrRepr::View(x.clone())));
        }

        // Make reduction_len
        let mut reduction_len = 1.;
        for &axis in axes.iter() {
            reduction_len *= x_shape[axis as usize] as f32;
        }
        // Do summation
        let sum = compute_reduce_sum(x, axes, self.keep_dims);

        // Do division
        let ret = match sum {
            crate::ArrRepr::Owned(mut arr) => {
                let reduction_len_inv = T::one() / T::from(reduction_len).unwrap();
                arr.mapv_inplace(move |elem| elem * reduction_len_inv);
                crate::ArrRepr::Owned(arr)
            }
            view @ _ => view,
        };

        ctx.push_output(Ok(ret))
    }

    fn grad(&self, gy: &'a Tensor<'a, T>, inputs: &[&'a Tensor<'a, T>], _: &'a Tensor<'a, T>, c: &mut Context<'a, T>) -> Vec<Option<&'a Tensor<'a, T>>> {
        let x = inputs[0];
        let axes = inputs[1];

        // Broadcast gy into x's shape
        let broadcast = Tensor::builder()
            .set_inputs(&[gy, c.shape(inputs[0]), inputs[1]])
            .build(c, ReduceGradCommon {
                should_make_broadcast_dims: !self.keep_dims,
                sparse_axes: self.sparse_axes,
            });

        // Divide
        let reduction_sizes = c.gather_common(c.shape(x), axes, 0);
        let reduction_len = c.reduce_prod(reduction_sizes, &[0], false);
        let gx = broadcast / reduction_len;

        vec![Some(gx), None]
    }
}

impl<'a, T: Float> op::Op<'a, T> for ReduceProd {
    fn name(&self) -> &str {
        "ReduceProd"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let x = &ctx.input(0);
        let axes = preprocess_axes(x, &ctx.input(1), self.sparse_axes);
        let ret = compute_reduce_prod(x, axes, self.keep_dims);
        ctx.push_output(Ok(ret));
    }

    fn grad(
        &self,
        gy: &'a Tensor<'a, T>,
        inputs: &[&'a Tensor<'a, T>],
        output: &'a Tensor<'a, T>,
        c: &mut Context<'a, T>
    ) -> Vec<Option<&'a Tensor<'a, T>>> {
        let grad_op = ReduceGradCommon {
            should_make_broadcast_dims: !self.keep_dims,
            sparse_axes: self.sparse_axes,
        };
        let tmp = Tensor::builder()
            .set_inputs(&[gy * output, c.shape(inputs[0]), inputs[1]])
            .build(c, grad_op);
        let gx = tmp / inputs[0];
        vec![Some(gx), None]
    }
}

impl<'a, T: Float> op::Op<'a, T> for ReduceMin {
    fn name(&self) -> &str {
        "ReduceMin"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let x = &ctx.input(0);
        let axes = preprocess_axes(x, &ctx.input(1), self.sparse_axes);
        ctx.push_output(Ok(compute_reduce_min(x, axes, self.keep_dims)));
    }

    fn grad(
        &self,
        gy: &'a Tensor<'a, T>,
        inputs: &[&'a Tensor<'a, T>],
        output: &'a Tensor<'a, T>,
        c: &mut Context<'a, T>
    ) -> Vec<Option<&'a Tensor<'a, T>>> {
        min_max_grad(gy, inputs, output, self.keep_dims, self.sparse_axes, c)
    }
}

impl<'a, T: Float> op::Op<'a, T> for ReduceMax {
    fn name(&self) -> &str {
        "ReduceMax"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let x = &ctx.input(0);
        let axes = preprocess_axes(x, &ctx.input(1), self.sparse_axes);
        ctx.push_output(Ok(compute_reduce_max(x, axes, self.keep_dims)));
    }

    fn grad(
        &self,
        gy: &'a Tensor<'a, T>,
        inputs: &[&'a Tensor<'a, T>],
        output: &'a Tensor<'a, T>,
        c: &mut Context<'a, T>
    ) -> Vec<Option<&'a Tensor<'a, T>>> {
        min_max_grad(gy, inputs, output, self.keep_dims, self.sparse_axes, c)
    }
}

fn min_max_grad<'a, T: Float>(
    gy: &'a Tensor<'a, T>,
    inputs: &[&'a Tensor<'a, T>],
    output: &'a Tensor<'a, T>,
    keep_dims: bool,
    sparse_axes: bool,
    c: &mut Context<'a, T>
) -> Vec<Option<&'a Tensor<'a, T>>> {
    let grad_op1 = ReduceGradCommon {
        should_make_broadcast_dims: !keep_dims,
        sparse_axes,
    };
    let grad_op2 = ReduceGradCommon {
        should_make_broadcast_dims: !keep_dims,
        sparse_axes,
    };
    let x = inputs[0];
    let x_shape = c.shape(inputs[0]);
    let y = Tensor::builder()
        .set_inputs(&[output, &x_shape, inputs[1]])
        .build(c, grad_op1);
    let gy = Tensor::builder()
        .set_inputs(&[gy, &x_shape, inputs[1]])
        .build(c, grad_op2);
    let eq = c.equal(&x, &y);
    vec![Some(c.mul(eq, &gy)), None]
}

impl<'a, T: Float> op::Op<'a, T> for ArgMax {
    fn name(&self) -> &str {
        "ArgMax"
    }

    // cf. https://github.com/tensorflow/compiler/tf2xla/kernels/index_ops.cc
    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        let x = &ctx.input(0);
        let axis = ndarray_ext::normalize_negative_axis(self.axis, x.ndim());
        let x_shape = x.shape();

        // 1. Make binary mask tensor (maximums are 1s)
        let mut mask = {
            let max_fn = T::max;
            let min_val = T::min_value();
            let maxed = x.fold_axis(ndarray::Axis(axis), min_val, move |&a, &b| max_fn(a, b));
            let mut mask = x.to_owned();
            let mut found = ndarray::Array::<bool, ndarray::IxDyn>::from_elem(maxed.shape(), false);
            for mut sub in mask.axis_iter_mut(ndarray::Axis(axis)) {
                ndarray::Zip::from(&mut sub)
                    .and(&mut found)
                    .and(&maxed)
                    .apply(|r, f, m| {
                        let z = r == m && !*f;
                        *f = z;
                        *r = T::from(z as i32).unwrap();
                    });
            }
            mask
        };

        // 2. Reshape the mask to 2-ranked. e.g. (2, 3, 4) -> (8, 3) (let `axis` be 1)
        let mask = {
            // move the `axis` to first, and put remaining together on the 2nd axis
            let reduction_len = x_shape[axis];
            ndarray_ext::roll_axis(&mut mask, ndarray::Axis(0), ndarray::Axis(axis));
            let shape2d = (reduction_len, mask.len() / reduction_len);
            let mut mask = mask.into_shape(shape2d).unwrap();
            mask.swap_axes(0, 1);
            mask
        };

        // 3. Make the indices (vertical vector)
        let indices = {
            let cols = mask.shape()[1];
            ndarray::Array::range(T::zero(), T::from(cols).unwrap(), T::one())
                .into_shape((cols, 1))
                .unwrap()
        };

        // 4. Dot product between mask and index-tensor
        let mat = mask.dot(&indices);

        // 5. Reshape it
        let result = {
            let mut final_shape = x_shape.to_vec();
            if self.keep_dim {
                final_shape[axis] = 1;
            } else {
                final_shape.remove(axis);
            }
            // unwrap is safe (95% confidence...)
            mat.into_dyn()
                .into_shape(ndarray::IxDyn(final_shape.as_slice()))
                .unwrap()
        };

        ctx.push_output(Ok(crate::ArrRepr::Owned(result)));
    }

    fn grad(&self, _: &'a Tensor<'a, T>, _: &[&'a Tensor<'a, T>], _: &'a Tensor<'a, T>, c: &mut Context<'a, T>) -> Vec<Option<&'a Tensor<'a, T>>> {
        vec![None]
    }
}

impl<'a, T: Float> op::Op<'a, T> for ReduceGradCommon {
    fn name(&self) -> &str {
        "ReduceGradCommon"
    }

    fn compute(&self, ctx: &mut crate::runtime::OpComputeContext<T>) {
        //  broadcast `gy` into `target_shape`
        let gy = &ctx.input(0);
        let target_shape = ndarray_ext::as_shape(&ctx.input(1)); // x's shape

        if gy.shape() == target_shape.as_slice() {
            return ctx.push_output(Ok(crate::ArrRepr::View(gy.clone())));
        }

        let x_is_scalar = ndarray_ext::is_scalar_shape(gy.shape());

        let ret = {
            let mut gy_view = gy.view();

            // make broadcast dims if needed
            if self.should_make_broadcast_dims || x_is_scalar {
                let axes = &ctx.input(2);

                // convert axes to usize vec
                let mut axes = if self.sparse_axes {
                    ndarray_ext::sparse_to_dense(axes)
                } else {
                    ndarray_ext::normalize_negative_axes(axes, target_shape.len())
                };

                let mut gy_shape = gy.shape().to_vec();
                axes.sort();
                for &axis in axes.iter() {
                    assert!(
                        axis <= gy_shape.len(),
                        "Bad gradient. You may passed a non-scalar value to `ag::grad`?"
                    );
                    gy_shape.insert(axis, 1);
                }
                gy_view = gy_view.into_shape(gy_shape).unwrap()
            }

            // do broadcast
            if let Some(ret) = gy_view.broadcast(target_shape) {
                ret.to_owned()
            } else {
                panic!("Bad gradient. You may passed a non-scalar value to `ag::grad`?")
            }
        };

        ctx.push_output(Ok(crate::ArrRepr::Owned(ret)));
    }

    fn grad(&self, gy: &'a Tensor<'a, T>, inputs: &[&'a Tensor<'a, T>], _: &'a Tensor<'a, T>, c: &mut Context<'a, T>) -> Vec<Option<&'a Tensor<'a, T>>> {
        let sum = ops::reduction_ops::ReduceSum {
            keep_dims: self.should_make_broadcast_dims,
            sparse_axes: self.sparse_axes,
        };
        let axes = inputs[2];
        let gx = Tensor::builder().set_inputs(&[gy, axes]).build(c, sum);
        vec![Some(gx), None, None]
    }
}
