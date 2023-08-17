use futures::{Stream, StreamExt};
use futures_concurrency::stream::Merge;
use tokio_util::sync::CancellationToken;

pub fn cancellation_stream<'a, T, S>(
    stream: S,
    cancel_token: CancellationToken,
) -> impl Stream<Item = Option<T>>
where
    S: Stream<Item = T> + Send + 'a,
    T: 'a,
{
    let this = stream.map(Option::Some);
    let cancellation = futures::stream::once(cancel_token.cancelled_owned()).map(|_| Option::None);
    (this, cancellation).merge()
}
