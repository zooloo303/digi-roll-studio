// SPDX-License-Identifier: GPL-3.0-or-later
#include <juce_audio_utils/juce_audio_utils.h>
class Fixture final : public juce::AudioProcessor {
public:
    Fixture() : AudioProcessor(BusesProperties().withOutput("Main", juce::AudioChannelSet::stereo(), true)) {
        addParameter(level = new juce::AudioParameterFloat("level", "Level", 0.0f, 1.0f, 0.25f));
        addParameter(telemetry = new juce::AudioParameterBool("transport", "Transport probe", false));
    }
    const juce::String getName() const override { return "DRS Host Fixture"; }
    void prepareToPlay(double, int) override {} void releaseResources() override {}
    void processBlock(juce::AudioBuffer<float>& b, juce::MidiBuffer& m) override {
        b.clear();
        if (telemetry->get()) {
            auto pos = getPlayHead()->getPosition();
            for (int n=0;n<b.getNumSamples();++n) {
                b.setSample(0,n,float(pos->getTimeInSeconds().orFallback(-1))/1024.0f);
                b.setSample(1,n,float(pos->getPpqPosition().orFallback(-1))/1024.0f);
            }
            return;
        }
        // An impulse encodes channel and velocity; no oscillator/firmware ambiguity.
        for (auto e : m) if (e.getMessage().isNoteOn())
            for (int c=0;c<b.getNumChannels();++c)
                b.addSample(c,e.samplePosition,level->get()*e.getMessage().getChannel()/16.0f);
    }
    bool acceptsMidi() const override { return true; } bool producesMidi() const override { return false; }
    double getTailLengthSeconds() const override { return 0; }
    bool hasEditor() const override { return true; }
    juce::AudioProcessorEditor* createEditor() override { return new juce::GenericAudioProcessorEditor(*this); }
    int getNumPrograms() override { return 1; } int getCurrentProgram() override { return 0; }
    void setCurrentProgram(int) override {} const juce::String getProgramName(int) override { return {}; }
    void changeProgramName(int,const juce::String&) override {}
    void getStateInformation(juce::MemoryBlock& b) override { float v=level->get(); b.replaceWith(&v,sizeof v); }
    void setStateInformation(const void* p,int n) override { if(n==sizeof(float)) { float v; std::memcpy(&v,p,sizeof v); *level=v; } }
private: juce::AudioParameterFloat* level; juce::AudioParameterBool* telemetry;
};
juce::AudioProcessor* JUCE_CALLTYPE createPluginFilter() { return new Fixture; }
