video_display_id = nil

function io_update_cb(identifier, entry)
    if video_display_id == nil and entry.type == "display" and entry.default then
        video_display_id = identifier
    end
end

function muxer_stats(stats)
    statusline = "Encoding, bitrate: " .. math.floor(stats.bitrate / 1000) .. " Kbps"
    tx.set_status(statusline)
end

function main(...)
    local arg = {...}
    rtp_host, rtp_port, yuv444, sdp_file = arg[1], arg[2], arg[3], arg[4]

    event = tx.register_io_cb(io_update_cb)
    event.destroy()

    tx.set_epoch(0)

    --[[ VIDEO ]] --
    source_video = tx.create_io(video_display_id, {
        width = 1920,
        height = 1080,
    })

    filter_vid_x264 = tx.create_filtergraph({
        graph = "scale=w=1920:h=-1,format=nv12",
        priv_options = { dump_graph = false },
    })

    local libx264 = {
        encoder = "libx264",
        options = {
            preset = "ultrafast",
            tune = "zerolatency",
            flags = "+global_header",
        }
    }

    local function nvenc_get_profile(yuv444)
        if yuv444 == "true" then
            return "high444p"
        else
            return "high"
        end
    end

    local nvenc = {
        encoder = "h264_nvenc",
        priv_options = { fifo_size = 1 },
        options = {
            preset = "p2",
            tune = "ull",
            zerolatency = true,
            delay = 1,
            profile = nvenc_get_profile(yuv444),
            b = "20M",
            flags = "+global_header",
        }
    }

    local vaapi = {
        encoder = "h264_vaapi",
        pix_fmt = "nv12",
        priv_options = { fifo_size = 2 },
        options = {
            profile = "high",
            b = "20M",
            bf = 0,
            flags = "+global_header",
        }
    }

    --[[
    encoder_v = tx.create_encoder(libx264)
    filter_vid_x264.link(source_video)
    encoder_v.link(filter_vid_x264)
    ]]

    --encoder_v = tx.create_encoder(vaapi)
    encoder_v = tx.create_encoder(nvenc)
    encoder_v.link(source_video)

    priv_options = {
        dump_info = true,
        low_latency = true,
    }

    if sdp_file ~= nil then
        priv_options.sdp_file = sdp_file
    end

    muxer = tx.create_muxer({
        out_url = string.format("rtp://%s:%s", rtp_host, rtp_port),
        out_format = "rtp",
        options = {
            max_interleave_delta = 0,
        },
        priv_options = priv_options,
    })
    muxer.link(encoder_v)
    muxer.schedule("stats", muxer_stats)

    tx.commit()
end
