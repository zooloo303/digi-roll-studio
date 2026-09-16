// SPDX-License-Identifier: GPL-3.0-or-later
// P1 standalone harness. No Rust, MIDI ports, scanner, or production IPC.
#include <juce_audio_utils/juce_audio_utils.h>
#include <atomic>
#include <thread>
#include <chrono>
#include <vector>
#include <iostream>
#include <cmath>

using namespace juce;
static var object() { return var(new DynamicObject); }
static void put(var& o, const Identifier& k, var v) { o.getDynamicObject()->setProperty(k,v); }
static void require(bool ok, const String& why) { if (!ok) throw std::runtime_error(why.toStdString()); }
struct Event { int64_t sample; int instance; MidiMessage message; };
struct ParameterEvent { int64_t sample; AudioProcessorParameter* parameter; float value; };
struct Transport { int64_t sample; double ppq, bpm; bool playing; };
class Playhead final : public AudioPlayHead {
public:
    PositionInfo position;
    Optional<PositionInfo> getPosition() const override { return position; }
};
class Editor final : public DocumentWindow {
public:
    explicit Editor(AudioPluginInstance& p) : DocumentWindow(p.getName(), Colours::darkgrey, allButtons) {
        setUsingNativeTitleBar(true); setContentOwned(p.createEditorIfNeeded(),true);
        centreWithSize(getWidth(),getHeight()); setVisible(true);
    }
    void closeButtonPressed() override { setVisible(false); }
};
struct Instance {
    std::unique_ptr<AudioPluginInstance> plugin;
    std::unique_ptr<Editor> editor;
    AudioBuffer<float> buffer;
    MidiBuffer midi;
    PluginDescription description;
};
// Persistent workers: no thread creation or allocation during a callback.
// Joining each block preserves same-frame mixing (no extra buffering latency).
// WaitableEvent can block: this remains an experimental host, not a hard-RT guarantee.
class RenderWorker final : public Thread {
public:
    explicit RenderWorker(Instance& target) : Thread("DRS plugin render"), instance(target) {}
    ~RenderWorker() override { signalThreadShouldExit(); ready.signal(); stopThread(-1); }
    void submit(int count) { samples=count; ready.signal(); }
    void finish() { finished.wait(-1); }
    void run() override {
        for (;;) {
            ready.wait(-1);
            if (threadShouldExit()) return;
            const ScopedNoDenormals noDenormals;
            AudioBuffer<float> b(instance.buffer.getArrayOfWritePointers(),instance.buffer.getNumChannels(),samples);
            instance.plugin->processBlock(b,instance.midi);
            finished.signal();
        }
    }
private:
    Instance& instance; int samples=0; WaitableEvent ready,finished;
};
class Host final : public JUCEApplication, private Timer, private AudioIODeviceCallback {
public:
    const String getApplicationName() override { return "DRS Plugin Host Preview"; }
    const String getApplicationVersion() override { return "0.1.0"; }
    bool moreThanOneInstanceAllowed() override { return true; }
    void initialise(const String& args) override {
        try {
            auto tokens=StringArray::fromTokens(args,true); tokens.removeEmptyStrings();
            require(tokens.size()==1,"Usage: drs-plugin-host /absolute/scenario.json");
            config=JSON::parse(File(tokens[0].unquoted()));
            require(config.isObject(),"Invalid scenario JSON");
            rate=double(config.getProperty("rate",48000)); block=int(config.getProperty("block",256));
            double seconds=double(config.getProperty("seconds",10));
            require(std::isfinite(rate) && rate>=8000 && rate<=192000 && block>=16 && block<=4096
                    && std::isfinite(seconds) && seconds>0 && seconds<=120,"Invalid render configuration (max 120 seconds)");
            measurementStart=double(config.getProperty("measurementStartSeconds",0));
            require(std::isfinite(measurementStart) && measurementStart>=0 && measurementStart<seconds,"Invalid measurement start");
            warmupSeconds=double(config.getProperty("warmupSeconds",0));
            require(std::isfinite(warmupSeconds) && warmupSeconds>=0 && warmupSeconds<=30,"Invalid warmup (0..30 seconds)");
            total=int64_t(seconds*rate); capture.setSize(2,int(total)); capture.clear();
            output=File(config["output"].toString()); require(output.isDirectory(),"Output directory must already exist");
            // Direct invocation also defaults third-party data to this local proof tree.
            if (SystemStats::getEnvironmentVariable("GEARMULATOR_DATA_ROOT", {}).isEmpty()) {
                auto data=output.getChildFile("gearmulator");
                require(data.createDirectory().wasOk(),"Cannot create isolated plugin data directory");
                require(setenv("GEARMULATOR_DATA_ROOT",data.getFullPathName().toRawUTF8(),1)==0,"Cannot select isolated plugin data root");
            }
            launchTicks=Time::getHighResolutionTicks();
            formats.addFormat(new VST3PluginFormat);
            auto* list=config["plugins"].getArray(); require(list && list->size()>0 && list->size()<=8,"Expected 1..8 plugins");
            for(auto spec:*list) {
                auto i=std::make_unique<Instance>(); OwnedArray<PluginDescription> found;
                formats.getFormat(0)->findAllTypesForFile(found,spec["path"].toString());
                require(found.size()==1,"Expected exactly one VST3 instrument: "+spec["path"].toString());
                i->description=*found[0]; String error;
                i->plugin=formats.createPluginInstance(i->description,rate,block,error);
                require(i->plugin!=nullptr,"Load failed: "+error);
                auto& p=*i->plugin;
                auto layout=p.getBusesLayout();
                for(auto& bus:layout.inputBuses) bus=AudioChannelSet::disabled();
                for(int b=0;b<layout.outputBuses.size();++b) layout.outputBuses.set(b,b==0?AudioChannelSet::stereo():AudioChannelSet::disabled());
                require(p.setBusesLayout(layout),"Plugin rejected main stereo-only layout");
                p.setPlayHead(&playhead); p.setRateAndBufferSizeDetails(rate,block);
                if(spec.hasProperty("stateIn")) { MemoryBlock state; require(File(spec["stateIn"].toString()).loadFileAsData(state),"Cannot read state"); p.setStateInformation(state.getData(),int(state.getSize())); }
                if(auto* changes=spec["parameters"].getArray()) for(auto change:*changes) {
                    auto index=int(change["index"]); auto value=float(change["value"]);
                    require(index>=0 && index<p.getParameters().size() && std::isfinite(value) && value>=0 && value<=1,"Invalid parameter");
                    p.getParameters()[index]->setValueNotifyingHost(value);
                }
                p.prepareToPlay(rate,block);
                i->buffer.setSize(jmax(2,p.getTotalNumOutputChannels()),block); i->midi.ensureSize(65536);
                if(bool(config.getProperty("editors",false))) i->editor=std::make_unique<Editor>(p);
                instances.push_back(std::move(i));
            }
            if(auto* events=config["events"].getArray()) for(auto e:*events) {
                auto t=int64_t(e["sample"]); auto index=int(e["instance"]); auto ch=int(e["channel"]);
                if(e.hasProperty("cc")) {
                    auto cc=int(e["cc"]),value=int(e["value"]);
                    require(t>=0 && t<total && index>=0 && index<int(instances.size()) && ch>=1 && ch<=16 && cc>=0 && cc<128 && value>=0 && value<128,"Invalid MIDI CC event");
                    schedule.push_back({t,index,MidiMessage::controllerEvent(ch,cc,value)});
                    continue;
                }
                if(e.hasProperty("sysex")) {
                    auto* bytes=e["sysex"].getArray();
                    require(t>=0 && t<total && index>=0 && index<int(instances.size()) && bytes && bytes->size()>0 && bytes->size()<=1024,"Invalid SysEx event");
                    MemoryBlock data;
                    for(auto byte:*bytes) { auto value=int(byte); require(value>=0 && value<128,"SysEx payload must be 7-bit (omit F0/F7)"); uint8 v=uint8(value); data.append(&v,1); }
                    schedule.push_back({t,index,MidiMessage::createSysExMessage(data.getData(),int(data.getSize()))});
                    continue;
                }
                auto note=int(e["note"]); auto velocity=int(e["velocity"]);
                require(t>=0 && t<total && index>=0 && index<int(instances.size()) && ch>=1 && ch<=16 && note>=0 && note<=127 && velocity>=0 && velocity<=127,"Invalid MIDI event");
                schedule.push_back({t,index,velocity?MidiMessage::noteOn(ch,note,uint8(velocity)):MidiMessage::noteOff(ch,note)});
            }
            require(schedule.size()<=4096,"Schedule capacity is 4096 events");
            for(size_t ix=0;ix<instances.size();++ix) {
                size_t bytes=65536;
                for(const auto& e:schedule) if(e.instance==int(ix)) bytes+=size_t(e.message.getRawDataSize())+32;
                instances[ix]->midi.ensureSize(bytes);
            }
            std::stable_sort(schedule.begin(),schedule.end(),[](auto& a,auto& b){return a.sample<b.sample;});
            if(auto* events=config["parameterEvents"].getArray()) for(auto e:*events) {
                auto sample=int64_t(e["sample"]); auto instance=int(e["instance"]); auto value=float(e["value"]);
                require(sample>=0 && sample<total && sample%block==0 && instance>=0 && instance<int(instances.size())
                    && std::isfinite(value) && value>=0 && value<=1,"Invalid parameter event");
                AudioProcessorParameter* target=nullptr;
                for(auto* p:instances[size_t(instance)]->plugin->getParameters())
                    if(auto* hosted=dynamic_cast<HostedAudioProcessorParameter*>(p))
                        if(hosted->getParameterID()==e["id"].toString()) target=p;
                require(target!=nullptr,"Unknown native parameter ID");
                parameterSchedule.push_back({sample,target,value});
            }
            require(parameterSchedule.size()<=4096,"Parameter schedule capacity is 4096");
            std::stable_sort(parameterSchedule.begin(),parameterSchedule.end(),[](auto& a,auto& b){return a.sample<b.sample;});
            transport.push_back({0,0,120,false});
            if(auto* states=config["transport"].getArray()) for(auto t:*states) {
                Transport s {int64_t(t["sample"]),double(t["ppq"]),double(t["bpm"]),bool(t["playing"])};
                require(s.sample>=0 && s.sample<total && s.sample%block==0 && std::isfinite(s.ppq) && std::isfinite(s.bpm) && s.bpm>0 && s.bpm<=1000,"Invalid transport (changes must align to blocks)"); transport.push_back(s);
            }
            std::stable_sort(transport.begin(),transport.end(),[](auto& a,auto& b){return a.sample<b.sample;});
            parallel=bool(config.getProperty("parallel",false));
            if(parallel) for(size_t ix=1;ix<instances.size();++ix) {
                auto w=std::make_unique<RenderWorker>(*instances[ix]);
                require(w->startRealtimeThread(Thread::RealtimeOptions{}.withApproximateAudioProcessingTime(block,rate)),"Cannot start realtime render worker");
                renderWorkers.push_back(std::move(w));
            }
            blockTimes.resize(size_t(total/16+1));
            // Keep the message loop available for plugin boot/editor work. No device
            // is open yet, and no authored events or render frames are consumed.
            worker=std::thread([this] {
                const auto begin=Time::getHighResolutionTicks();
                const auto pacingStart=std::chrono::steady_clock::now();
                playhead.position.setTimeInSamples(0); playhead.position.setTimeInSeconds(0);
                playhead.position.setPpqPosition(0); playhead.position.setBpm(120);
                playhead.position.setIsPlaying(false);
                playhead.position.setTimeSignature(AudioPlayHead::TimeSignature{4,4});
                for(int64_t frame=0;double(frame)<warmupSeconds*rate && !cancel.load();frame+=block) {
                    const auto tick=Time::getHighResolutionTicks();
                    for(auto& i:instances) { i->buffer.clear(); i->midi.clear(); }
                    processInstances(block);
                    const auto elapsed=Time::highResolutionTicksToSeconds(Time::getHighResolutionTicks()-tick);
                    ++warmupBlocks; warmupMax=jmax(warmupMax,elapsed);
                    if(elapsed>double(block)/rate) ++warmupOverBudget;
                    // Firmware/editor startup can also depend on wall-clock timers.
                    // Accelerated emulation alone does not settle those callbacks.
                    std::this_thread::sleep_until(pacingStart+std::chrono::duration<double>(double(frame+block)/rate));
                }
                warmupWall=Time::highResolutionTicksToSeconds(Time::getHighResolutionTicks()-begin);
                prepared=true;
            });
            startTimer(50);
        } catch(const std::exception& e) { fail(e.what()); }
    }
    void shutdown() override {
        stopTimer(); cancel=true; device.removeAudioCallback(this); device.closeAudioDevice();
        if(worker.joinable()) worker.join();
        renderWorkers.clear();
        for(auto& i:instances) { i->editor.reset(); i->plugin->releaseResources(); i->plugin->setPlayHead(nullptr); }
        instances.clear();
    }
private:
    void fail(const String& message) { std::cerr<<message<<std::endl; setApplicationReturnValue(1); quit(); }
    void audioDeviceAboutToStart(AudioIODevice*) override {}
    void audioDeviceStopped() override {}
    void audioDeviceIOCallbackWithContext(const float* const*,int,float* const* out,int channels,int count,const AudioIODeviceCallbackContext&) override {
        for(int c=0;c<channels;++c) if(out[c]) FloatVectorOperations::clear(out[c],count);
        if(!done.load() && !cancel.load()) render(out,channels,count);
    }
    void processInstances(int count) {
        const ScopedNoDenormals noDenormals;
        for(auto& w:renderWorkers) w->submit(count);
        const auto direct=parallel?size_t(1):instances.size();
        for(size_t ix=0;ix<direct;++ix) {
            auto& i=instances[ix];
            AudioBuffer<float> b(i->buffer.getArrayOfWritePointers(),i->buffer.getNumChannels(),count);
            i->plugin->processBlock(b,i->midi);
        }
        for(auto& w:renderWorkers) w->finish();
    }
    void startRendering() {
        if(config.hasProperty("device")) {
            AudioDeviceManager::AudioDeviceSetup setup;
            setup.outputDeviceName=config["device"].toString();
            setup.inputDeviceName={}; setup.sampleRate=rate; setup.bufferSize=block;
            auto error=device.initialise(0,2,nullptr,false,{},&setup);
            require(error.isEmpty(),error);
            auto* d=device.getCurrentAudioDevice(); require(d && d->getCurrentSampleRate()==rate && d->getCurrentBufferSizeSamples()==block,"Device did not accept requested rate/block");
            deviceLatency=d->getOutputLatencyInSamples();
            renderStartTicks=Time::getHighResolutionTicks();
            device.addAudioCallback(this);
        } else { renderStartTicks=Time::getHighResolutionTicks(); worker=std::thread([this]{ while(!done.load() && !cancel.load()) render(nullptr,0,block); }); }
    }
    void render(float* const* out,int channels,int count) {
        if(count>block) { badBlock=true; done=true; return; }
        const ScopedNoDenormals noDenormals;
        auto started=Time::getHighResolutionTicks();
        auto n=int(jmin(int64_t(count),total-cursor));
        while(transportIndex+1<transport.size() && transport[transportIndex+1].sample<=cursor) ++transportIndex;
        auto t=transport[transportIndex];
        const auto musicalSeconds=t.ppq*60.0/t.bpm+(t.playing?double(cursor-t.sample)/rate:0.0);
        playhead.position.setTimeInSamples(int64_t(std::llround(musicalSeconds*rate)));
        playhead.position.setTimeInSeconds(musicalSeconds);
        playhead.position.setBpm(t.bpm); playhead.position.setIsPlaying(t.playing);
        playhead.position.setPpqPosition(t.ppq+(t.playing?double(cursor-t.sample)/rate*t.bpm/60:0));
        playhead.position.setTimeSignature(AudioPlayHead::TimeSignature{4,4});
        for(auto& i:instances) { i->buffer.clear(); i->midi.clear(); }
        while(nextEvent<schedule.size() && schedule[nextEvent].sample<cursor+n) {
            auto& e=schedule[nextEvent++]; instances[size_t(e.instance)]->midi.addEvent(e.message,int(e.sample-cursor));
        }
        while(nextParameter<parameterSchedule.size() && parameterSchedule[nextParameter].sample<=cursor) {
            auto& e=parameterSchedule[nextParameter++]; e.parameter->setValueNotifyingHost(e.value);
        }
        processInstances(n);
        for(auto& i:instances)
            for(int c=0;c<2;++c) capture.addFrom(c,int(cursor),i->buffer,c,0,n,0.25f);
        if(out) for(int c=0;c<jmin(channels,2);++c) if(out[c]) FloatVectorOperations::copy(out[c],capture.getReadPointer(c,int(cursor)),n);
        cursor+=n; publishedCursor.store(cursor,std::memory_order_relaxed); ++blocks;
        auto duration=Time::highResolutionTicksToSeconds(Time::getHighResolutionTicks()-started);
        blockTimes[size_t(blocks-1)]=duration;
        if(double(cursor-n)/rate>=measurementStart) {
            ++measuredBlocks; measuredSeconds+=duration; measuredMax=jmax(measuredMax,duration);
            if(duration>double(count)/rate) ++measuredOverBudget;
        }
        renderSeconds+=duration; maxBlockSeconds=jmax(maxBlockSeconds,duration);
        if(duration>double(count)/rate) ++overBudget;
        if(cursor>=total) done=true;
    }
    void timerCallback() override {
        if(!rendering) {
            if(!prepared.load()) return;
            worker.join();
            try { startRendering(); rendering=true; }
            catch(const std::exception& e) { stopTimer(); fail(e.what()); }
            return;
        }
        if(measurementXruns<0 && double(publishedCursor.load(std::memory_order_relaxed))/rate>=measurementStart)
            if(auto* d=device.getCurrentAudioDevice()) {
                measurementXruns=d->getXRunCount();
                xrunMeasurementFrame=publishedCursor.load(std::memory_order_relaxed);
            }
        if(!done.load()) return;
        stopTimer(); device.removeAudioCallback(this); if(worker.joinable()) worker.join();
        auto endTicks=Time::getHighResolutionTicks();
        try {
            require(!badBlock,"Unexpected oversized device block");
            var report=object();
            put(report,"parallel",parallel);
            put(report,"measurementStartSeconds",measurementStart);
            const auto finalXruns=device.getCurrentAudioDevice()?device.getCurrentAudioDevice()->getXRunCount():-1;
            put(report,"deviceXrunsAfterMeasurementStart",measurementXruns>=0 && finalXruns>=measurementXruns?finalXruns-measurementXruns:-1);
            put(report,"xrunMeasurementStartFrame",int64(xrunMeasurementFrame));
            put(report,"measuredBlocks",int64(measuredBlocks));
            put(report,"measuredRenderSeconds",measuredSeconds);
            put(report,"measuredMaxBlockSeconds",measuredMax);
            put(report,"measuredBlocksOverBudget",int64(measuredOverBudget));
            Array<var> misses;
            for(int64_t ix=0;ix<blocks;++ix) if(blockTimes[size_t(ix)]>double(block)/rate) {
                var miss=object(); put(miss,"frame",int64(ix*block));
                put(miss,"seconds",blockTimes[size_t(ix)]); misses.add(miss);
            }
            put(report,"deadlineMisses",misses);
            put(report,"warmupSeconds",warmupSeconds); put(report,"warmupWallSeconds",warmupWall);
            put(report,"warmupBlocks",int64(warmupBlocks)); put(report,"warmupBlocksOverBudget",int64(warmupOverBudget));
            put(report,"warmupMaxBlockSeconds",warmupMax);
            blockTimes.resize(size_t(blocks)); std::sort(blockTimes.begin(),blockTimes.end());
            put(report,"p50BlockSeconds",blockTimes[size_t((blocks-1)*.50)]);
            put(report,"p99BlockSeconds",blockTimes[size_t((blocks-1)*.99)]);
            put(report,"callbackBudgetSeconds",double(block)/rate);
            put(report,"startupSeconds",Time::highResolutionTicksToSeconds(renderStartTicks-launchTicks));
            put(report,"renderWallSeconds",Time::highResolutionTicksToSeconds(endTicks-renderStartTicks));
            put(report,"reportedDeviceOutputLatencySamples",deviceLatency);
            put(report,"deviceXruns",device.getCurrentAudioDevice()?device.getCurrentAudioDevice()->getXRunCount():-1); put(report,"rate",rate); put(report,"block",block); put(report,"frames",int64(cursor));
            put(report,"parameterEventsDelivered",int(nextParameter)); put(report,"eventsDelivered",int(nextEvent)); put(report,"blocks",int64(blocks));
            put(report,"renderSeconds",renderSeconds); put(report,"maxBlockSeconds",maxBlockSeconds);
            put(report,"blocksOverBudget",int64(overBudget)); put(report,"device",config.getProperty("device","offline"));
            put(report,"mixGainPerInstance",0.25); put(report,"peak",capture.getMagnitude(0,capture.getNumSamples()));
            Array<var> plugins;
            for(size_t ix=0;ix<instances.size();++ix) {
                auto& i=*instances[ix]; var entry=object(); auto& p=*i.plugin;
                put(entry,"name",p.getName()); put(entry,"version",i.description.version);
                put(entry,"identifier",i.description.createIdentifierString()); put(entry,"latencySamples",p.getLatencySamples());
                Array<var> parameters;
                for(auto* param:p.getParameters()) {
                    var v=object(); put(v,"index",param->getParameterIndex()); put(v,"name",param->getName(128));
                    auto* hosted=dynamic_cast<HostedAudioProcessorParameter*>(param);
                    put(v,"id",hosted?hosted->getParameterID():String()); put(v,"value",param->getValue()); parameters.add(v);
                }
                put(entry,"parameters",parameters);
                MemoryBlock state; p.getStateInformation(state);
                auto stateFile=output.getChildFile("instance-"+String(int(ix))+".state");
                require(stateFile.replaceWithData(state.getData(),state.getSize()),"Cannot save state");
                put(entry,"stateBytes",int64(state.getSize())); plugins.add(entry);
            }
            put(report,"plugins",plugins);
            auto stream=output.getChildFile("mix.wav").createOutputStream(); require(stream!=nullptr,"Cannot create WAV");
            stream->setPosition(0); stream->truncate(); WavAudioFormat wav;
            std::unique_ptr<AudioFormatWriter> writer(wav.createWriterFor(stream.get(),rate,2,24,{},0));
            require(writer!=nullptr,"Cannot create WAV writer"); stream.release();
            require(writer->writeFromAudioSampleBuffer(capture,0,capture.getNumSamples()),"Cannot write WAV"); writer.reset();
            require(output.getChildFile("report.json").replaceWithText(JSON::toString(report)),"Cannot write report");
            std::cout<<"Wrote "<<output.getChildFile("report.json").getFullPathName()<<std::endl; quit();
        } catch(const std::exception& e) { fail(e.what()); }
    }
    var config; File output; AudioPluginFormatManager formats; AudioDeviceManager device; Playhead playhead;
    std::vector<std::unique_ptr<Instance>> instances; std::vector<Event> schedule; std::vector<Transport> transport; std::vector<ParameterEvent> parameterSchedule;
    std::vector<std::unique_ptr<RenderWorker>> renderWorkers; std::vector<double> blockTimes; bool parallel=false;
    AudioBuffer<float> capture; std::thread worker; std::atomic<bool> done{false},cancel{false};
    double measurementStart=0,measuredSeconds=0,measuredMax=0; int64_t measuredBlocks=0,measuredOverBudget=0;
    std::atomic<bool> prepared{false}; bool rendering=false;
    double warmupSeconds=0,warmupWall=0,warmupMax=0; int64_t warmupBlocks=0,warmupOverBudget=0;
    bool badBlock=false; double rate=48000,renderSeconds=0,maxBlockSeconds=0; int block=256;
    std::atomic<int64_t> publishedCursor{0}; int measurementXruns=-1; int64_t xrunMeasurementFrame=-1;
    int deviceLatency=0; int64_t launchTicks=0,renderStartTicks=0;
    int64_t total=0,cursor=0,blocks=0,overBudget=0; size_t nextEvent=0,transportIndex=0,nextParameter=0;
};
START_JUCE_APPLICATION(Host)
