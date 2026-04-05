import { MessageTopic$Input } from "tdlib-types";

export type messageType = {
    /** 消息所属的聊天 ID */
    chat_id: number;
    /** 消息 ID */
    message_id: number;
    /**
     * @deprecated 已被 topic_id 取代
     *  线程 ID */
    thread_id?: number;
    /** 主题 ID */
    topic_id?: MessageTopic$Input;
    /** 消息链接 */
    link: string;
};

export type navMessageType = {
    /** 索引，从1开始，navMessage为主消息 */
    page: number;
    /** 消息所属的聊天 ID */
    chat_id: number;
    /** 消息 ID */
    message_id: number;
    /**
     * @deprecated 已被 topic_id 取代
     *  线程 ID */
    thread_id?: number;
    /** 主题 ID */
    topic_id?: MessageTopic$Input;
    /** 消息链接 */
    link: string;
};
/**
 * 分段/相册上传时每条消息的基本信息
 */
export type albumMessageType = {
    /** 消息所属的聊天 ID */
    chat_id: number;
    /** 消息 ID */
    message_id: number;
    /** 线程 ID */
    thread_id?: number;
    /** 主题 ID */
    topic_id?: MessageTopic$Input;
    /** TG 视频远程 ID */
    videoid?: string;
    /** TG 视频唯一 ID */
    unique_id?: string;
};
