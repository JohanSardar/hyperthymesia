use super::*;
use dispatch2::DispatchQueue;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::*;
use objc2_core_foundation::{CFDictionary, CFMutableData};
use objc2_core_image::{CIContext, CIImage};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSURL};
use objc2_image_io::CGImageDestination;
use objc2_screen_capture_kit::*;

/// SCK owns immutable IOSurface-backed samples. Retaining a sample extends its
/// lifetime across queues; consumers never mutate the shared pixel buffer.
pub struct Frame {
    sample: CFRetained<CMSampleBuffer>,
    pub nanos: u64,
}
unsafe impl Send for Frame {}
unsafe impl Sync for Frame {}
struct OutputState {
    frames: ArrayQueue<Frame>,
    dropped: AtomicU64,
}
define_class!(
    #[unsafe(super=NSObject)]
    #[ivars=Arc<OutputState>]
    struct Output;
    unsafe impl NSObjectProtocol for Output {}
    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn output(&self,_stream:&SCStream,sample:&CMSampleBuffer,kind:SCStreamOutputType) {
            if kind!=SCStreamOutputType::Screen {return;}
            if unsafe {sample.image_buffer()}.is_none() {return;}
            // SCK emits status-only/idle samples; these are not new visual evidence.
            let complete=unsafe {sample.sample_attachments_array(false)}.and_then(|array| {
                let array=unsafe {CFRetained::cast_unchecked::<CFArray<CFType>>(array)};
                let dictionary=array.get(0)?.downcast::<CFDictionary>().ok()?;
                let dictionary=unsafe {CFRetained::cast_unchecked::<CFDictionary<CFString,CFType>>(dictionary)};
                let key=unsafe {&*(SCStreamFrameInfoStatus as *const NSString).cast::<CFString>()};
                let value=dictionary.get(key)?;
                let number=value.downcast::<objc2_core_foundation::CFNumber>().ok()?;
                let number=unsafe {&*((&*number as *const objc2_core_foundation::CFNumber).cast::<NSNumber>())};
                Some(number.integerValue()==SCFrameStatus::Complete.0)
            }).unwrap_or(false);
            if !complete {return;}
            let sample=unsafe {CFRetained::retain(NonNull::from(sample))};
            if self.ivars().frames.push(Frame {sample,nanos:continuous_nanos()}).is_err(){self.ivars().dropped.fetch_add(1,Ordering::Relaxed);}
        }
    }
);
pub struct ScreenStream {
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    state: Arc<OutputState>,
    pub width: u32,
    pub height: u32,
}
fn wait_completion(
    call: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let block = RcBlock::new(move |err: *mut NSError| {
        let result = if err.is_null() {
            Ok(())
        } else {
            Err(unsafe { &*err }.to_string())
        };
        let _ = tx.try_send(result);
    });
    call(&block);
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?
}
impl ScreenStream {
    /// Capture a fixed foreground window on the main display. Inclusion filtering
    /// prevents another app appearing in the recording during activation races.
    pub fn start(focus: &Focus) -> Result<Self, String> {
        if !permissions().screen {
            return Err("PermissionDenied".into());
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let pid = focus.pid;
        let title = focus.title.clone();
        let block = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                let result = (|| -> Result<_, String> {
                    if content.is_null() {
                        return Err(if error.is_null() {
                            "no shareable content".into()
                        } else {
                            unsafe { &*error }.to_string()
                        });
                    }
                    unsafe {
                        let content = &*content;
                        let window = content
                            .windows()
                            .iter()
                            .find(|w| {
                                w.isOnScreen()
                                    && w.windowLayer() == 0
                                    && w.owningApplication().is_some_and(|a| a.processID() == pid)
                                    && title.as_ref().is_none_or(|t| {
                                        w.title().is_some_and(|v| v.to_string() == *t)
                                    })
                            })
                            .ok_or("no matching focused window")?;
                        let display = content
                            .displays()
                            .iter()
                            .find(|d| d.displayID() == CGMainDisplayID())
                            .ok_or("no main display")?;
                        // Only this window is included. A broader display view requires a
                        // continuously refreshed per-window privacy map.
                        let filter = SCContentFilter::initWithDisplay_includingWindows(
                            SCContentFilter::alloc(),
                            &display,
                            &NSArray::from_retained_slice(&[window]),
                        );
                        let width = display.width() as u32;
                        let height = display.height() as u32;
                        let config = SCStreamConfiguration::new();
                        config.setWidth(width as usize);
                        config.setHeight(height as usize);
                        config.setMinimumFrameInterval(CMTime {
                            value: 1,
                            timescale: 1,
                            flags: objc2_core_media::CMTimeFlags::Valid,
                            epoch: 0,
                        });
                        config.setQueueDepth(3);
                        config.setShowsCursor(false);
                        config.setCapturesAudio(false);
                        let stream = SCStream::initWithFilter_configuration_delegate(
                            SCStream::alloc(),
                            &filter,
                            &config,
                            None,
                        );
                        Ok((stream, width, height))
                    }
                })();
                let _ = tx.try_send(result);
            },
        );
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true,true,&block);
        }
        let (stream, width, height) = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())??;
        let state = Arc::new(OutputState {
            frames: ArrayQueue::new(3),
            dropped: AtomicU64::new(0),
        });
        let output: Retained<Output> = unsafe {
            let obj = Output::alloc().set_ivars(state.clone());
            msg_send![super(obj), init]
        };
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    SCStreamOutputType::Screen,
                    Some(&DispatchQueue::new("com.hyperthymesia.screen", None)),
                )
                .map_err(|e| e.to_string())?;
        }
        wait_completion(|cb| unsafe { stream.startCaptureWithCompletionHandler(Some(cb)) })?;
        Ok(Self {
            stream,
            _output: output,
            state,
            width,
            height,
        })
    }
    pub fn pop(&self) -> Option<Frame> {
        self.state.frames.pop()
    }
    pub fn dropped(&self) -> u64 {
        self.state.dropped.load(Ordering::Relaxed)
    }
}
impl Drop for ScreenStream {
    fn drop(&mut self) {
        let _ =
            wait_completion(|cb| unsafe { self.stream.stopCaptureWithCompletionHandler(Some(cb)) });
    }
}
impl Frame {
    pub fn jpeg(&self) -> Result<Vec<u8>, String> {
        unsafe {
            let pixel = self.sample.image_buffer().ok_or("no pixel buffer")?;
            let image = CIImage::imageWithCVPixelBuffer(&pixel);
            let context = CIContext::contextWithOptions(None);
            let cg = context
                .createCGImage_fromRect(&image, image.extent())
                .ok_or("CGImage conversion failed")?;
            let data = CFMutableData::new(None, 0).ok_or("image buffer allocation")?;
            let destination =
                CGImageDestination::with_data(&data, &CFString::from_str("public.jpeg"), 1, None)
                    .ok_or("JPEG destination")?;
            let key = &*(objc2_image_io::kCGImageDestinationLossyCompressionQuality
                as *const CFString)
                .cast::<NSString>();
            let quality = NSNumber::new_f64(0.92);
            let options = NSDictionary::from_slices(&[key], &[&*quality]);
            let properties =
                &*((&*options as *const NSDictionary<NSString, NSNumber>).cast::<CFDictionary>());
            destination.add_image(&cg, Some(properties));
            if !destination.finalize() {
                return Err("JPEG encoding failed".into());
            }
            Ok(data.to_vec())
        }
    }
}
pub struct VideoWriter {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    started: bool,
    pub frames: u32,
}
impl VideoWriter {
    pub fn new(path: &std::path::Path, width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(
                path.to_str().ok_or("invalid video path")?,
            ));
            let writer = AVAssetWriter::initWithURL_fileType_error(
                AVAssetWriter::alloc(),
                &url,
                AVFileTypeMPEG4.ok_or("missing AVFoundation constant")?,
            )
            .map_err(|e| e.to_string())?;
            let bitrate = NSNumber::new_i32(160_000);
            let compression = NSDictionary::from_slices(
                &[AVVideoAverageBitRateKey.ok_or("missing AVFoundation constant")?],
                &[&*bitrate],
            );
            let w = NSNumber::new_u32(width);
            let h = NSNumber::new_u32(height);
            let settings = NSDictionary::<NSString, objc2::runtime::AnyObject>::from_slices(
                &[
                    AVVideoCodecKey.ok_or("missing AVFoundation constant")?,
                    AVVideoWidthKey.ok_or("missing AVFoundation constant")?,
                    AVVideoHeightKey.ok_or("missing AVFoundation constant")?,
                    AVVideoCompressionPropertiesKey.ok_or("missing AVFoundation constant")?,
                ],
                &[
                    AVVideoCodecTypeHEVC.ok_or("HEVC unavailable")?.as_ref(),
                    w.as_ref(),
                    h.as_ref(),
                    compression.as_ref(),
                ],
            );
            let input = AVAssetWriterInput::initWithMediaType_outputSettings(
                AVAssetWriterInput::alloc(),
                AVMediaTypeVideo.ok_or("missing AVFoundation constant")?,
                Some(&settings),
            );
            input.setExpectsMediaDataInRealTime(true);
            if !writer.canAddInput(&input) {
                return Err("HEVC input unsupported".into());
            }
            writer.addInput(&input);
            if !writer.startWriting() {
                return Err(format!("writer start: {:?}", writer.error()));
            }
            Ok(Self {
                writer,
                input,
                started: false,
                frames: 0,
            })
        }
    }
    pub fn append(&mut self, frame: &Frame) -> Result<bool, String> {
        unsafe {
            if !self.started {
                self.writer
                    .startSessionAtSourceTime(frame.sample.presentation_time_stamp());
                self.started = true;
            }
            if !self.input.isReadyForMoreMediaData() {
                return Ok(false);
            }
            if !self.input.appendSampleBuffer(&frame.sample) {
                return Err(format!("writer append: {:?}", self.writer.error()));
            }
            self.frames += 1;
            Ok(true)
        }
    }
    pub fn finish(self) -> Result<(), String> {
        unsafe {
            self.input.markAsFinished();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let cb = RcBlock::new(move || {
                let _ = tx.try_send(());
            });
            self.writer.finishWritingWithCompletionHandler(&cb);
            rx.recv_timeout(Duration::from_secs(10))
                .map_err(|e| e.to_string())?;
            if self.writer.status() != AVAssetWriterStatus::Completed {
                return Err(format!("writer finish: {:?}", self.writer.error()));
            }
            Ok(())
        }
    }
}
