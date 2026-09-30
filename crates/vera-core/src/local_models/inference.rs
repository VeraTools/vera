//! Checks shared by local ONNX embedding and reranking inference.

use anyhow::Result;
use std::sync::{Mutex, MutexGuard};

pub(crate) fn lock_session<T>(session: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    session
        .lock()
        .map_err(|_| anyhow::anyhow!("ONNX session lock poisoned"))
}

/// Require one output row per input and a buffer matching its positive dimensions.
pub(crate) fn validate_batch_tensor(
    shape: &[i64],
    data_len: usize,
    batch_size: usize,
) -> Result<()> {
    anyhow::ensure!(
        shape.first().and_then(|dim| usize::try_from(*dim).ok()) == Some(batch_size),
        "ONNX output batch dimension does not match {batch_size} inputs: {shape:?}"
    );
    let tensor_len = shape.iter().try_fold(1usize, |size, &dim| {
        let dim = usize::try_from(dim).ok().filter(|dim| *dim > 0)?;
        size.checked_mul(dim)
    });
    anyhow::ensure!(
        tensor_len == Some(data_len),
        "ONNX output dimensions {shape:?} do not describe {data_len} values"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_poisoned_session_lock_returns_an_error() {
        let session = Arc::new(Mutex::new(()));
        let poisoned = Arc::clone(&session);
        assert!(
            std::thread::spawn(move || {
                let _guard = poisoned.lock().unwrap();
                panic!("poison the model session for the regression");
            })
            .join()
            .is_err()
        );
        let error = lock_session(&session).unwrap_err();
        assert!(error.to_string().contains("session lock poisoned"));
    }

    #[test]
    fn valid_session_lock_remains_usable() {
        let session = Mutex::new(17);
        assert_eq!(*lock_session(&session).unwrap(), 17);
    }
}
