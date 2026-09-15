cmake_minimum_required(VERSION 3.22)
project(DRSPluginHost VERSION 0.1.0 LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT APPLE)
  message(FATAL_ERROR "P1 currently supports macOS only")
endif()
find_package(Python3 REQUIRED COMPONENTS Interpreter)
include(FetchContent)
FetchContent_Declare(JUCE
  URL https://github.com/juce-framework/JUCE/archive/4f43011b96eb0636104cb3e433894cda98243626.tar.gz
  DOWNLOAD_EXTRACT_TIMESTAMP TRUE
  URL_HASH SHA256=efaeeed2ca988d3ade7b21dd01dca5c79fd9a2fb5884ea6140b31a7a87e61053)
FetchContent_GetProperties(JUCE)
if(NOT juce_POPULATED)
  FetchContent_Populate(JUCE)
endif()
execute_process(COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/patch-juce.py ${juce_SOURCE_DIR}
  COMMAND_ERROR_IS_FATAL ANY)
add_subdirectory(${juce_SOURCE_DIR} ${juce_BINARY_DIR})
juce_add_gui_app(drs-plugin-host PRODUCT_NAME "DRS Plugin Host Preview"
  BUNDLE_ID "studio.digi-roll.plugin-preview.host")
target_sources(drs-plugin-host PRIVATE src/Main.cpp)
target_compile_definitions(drs-plugin-host PRIVATE JUCE_PLUGINHOST_VST3=1
  JUCE_PLUGINHOST_AU=0 JUCE_WEB_BROWSER=0 JUCE_USE_CURL=0
  JUCE_APPLICATION_NAME_STRING="DRSPluginHostPreview" JUCE_APPLICATION_VERSION_STRING="0.1.0")
target_link_libraries(drs-plugin-host PRIVATE juce::juce_audio_utils)
juce_add_plugin(drs-host-fixture FORMATS VST3 PRODUCT_NAME "DRS Host Fixture"
  PLUGIN_MANUFACTURER_CODE Drsp PLUGIN_CODE Dspf IS_SYNTH TRUE NEEDS_MIDI_INPUT TRUE
  COPY_PLUGIN_AFTER_BUILD FALSE)
target_sources(drs-host-fixture PRIVATE tests/Fixture.cpp)
target_compile_definitions(drs-host-fixture PRIVATE JUCE_WEB_BROWSER=0 JUCE_USE_CURL=0 JUCE_VST3_CAN_REPLACE_VST2=0)
target_link_libraries(drs-host-fixture PRIVATE juce::juce_audio_utils)
enable_testing()
find_package(Python3 REQUIRED COMPONENTS Interpreter)
add_test(NAME host-proof COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/tests/proof.py
  $<TARGET_FILE:drs-plugin-host> $<TARGET_FILE_DIR:drs-host-fixture_VST3>/../..)
set_tests_properties(host-proof PROPERTIES TIMEOUT 60)
