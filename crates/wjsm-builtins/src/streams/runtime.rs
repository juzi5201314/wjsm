use wjsm_host::{
    ReadableStreamEntry, ReaderEntry, StreamControllerEntry, StreamState, Value,
    WritableStreamEntry, WritableStreamState, WriterEntry,
};

#[derive(Clone, Copy, Debug)]
pub enum PipeToAction {
    Write { destination: u32, chunk: Value },
    Close { destination: u32, promise: Value },
    Pull,
    Wait,
    Done,
}

pub fn next_pipe_to_action(
    stream: &mut ReadableStreamEntry,
    controller: Option<&mut StreamControllerEntry>,
) -> PipeToAction {
    let Some(pipe) = stream.pipe_to.as_mut() else {
        return PipeToAction::Done;
    };
    if pipe.write_in_flight || pipe.closing {
        return PipeToAction::Wait;
    }
    let close_requested = match controller {
        Some(controller) => {
            if let Some(chunk) = controller.chunk_queue.pop_front() {
                pipe.write_in_flight = true;
                return PipeToAction::Write {
                    destination: pipe.destination,
                    chunk,
                };
            }
            controller.close_requested
        }
        None => true,
    };
    if matches!(stream.state, StreamState::Closed) || close_requested {
        pipe.closing = true;
        PipeToAction::Close {
            destination: pipe.destination,
            promise: pipe.promise,
        }
    } else {
        PipeToAction::Pull
    }
}

pub fn finish_pipe_to_write(
    stream: &mut ReadableStreamEntry,
    error: Option<Value>,
) -> (Option<Value>, bool) {
    if error.is_some() {
        let promise = stream.pipe_to.map(|pipe| pipe.promise);
        stream.pipe_to = None;
        (promise, false)
    } else if let Some(pipe) = stream.pipe_to.as_mut() {
        pipe.write_in_flight = false;
        (None, true)
    } else {
        (None, false)
    }
}

pub fn clear_pipe_to(stream: &mut ReadableStreamEntry) {
    stream.pipe_to = None;
}

pub fn finish_writable_close<'a>(
    stream: &mut WritableStreamEntry,
    writers: impl Iterator<Item = &'a WriterEntry>,
    writable_stream_handle: u32,
) -> Vec<Value> {
    stream.state = WritableStreamState::Closed;
    writers
        .filter(|writer| writer.writable_stream_handle == writable_stream_handle)
        .filter_map(|writer| writer.closed_promise)
        .collect()
}

pub fn close_readable_after_flush(
    stream: &mut ReadableStreamEntry,
    controller: &mut StreamControllerEntry,
    readers: &mut [Option<ReaderEntry>],
    stream_handle: u32,
) -> Option<Value> {
    controller.close_requested = true;
    stream.state = StreamState::Closed;
    readers
        .iter_mut()
        .filter_map(Option::as_mut)
        .find(|reader| reader.stream_handle == stream_handle)
        .and_then(|reader| reader.pending_read_promise.take())
}
