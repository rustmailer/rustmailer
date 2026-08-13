/*
 * Copyright © 2025-2026 rustmailer.com
 * Licensed under RustMailer License Agreement v1.0
 * Unauthorized use or distribution is prohibited.
 */

import { OAuth2Entity } from "@/features/oauth2/data/schema";
import axiosInstance from "../axiosInstance";

export const get_oauth2_list = async () => {
    const response = await axiosInstance.get<{ items: OAuth2Entity[] }>("/api/v1/oauth2-list");
    return response.data;
};

export const delete_oauth2 = async (id: number) => {
    const response = await axiosInstance.delete(`/api/v1/oauth2/${id}`);
    return response.data;
};

export const create_oauth2 = async (data: Record<string, any>) => {
    const response = await axiosInstance.post<OAuth2Entity>("/api/v1/oauth2", data);
    return response.data;
};

export const update_oauth2 = async (id: number, data: Record<string, any>) => {
    const response = await axiosInstance.post<OAuth2Entity>(`/api/v1/oauth2/${id}`, data);
    return response.data;
};


export const get_authorize_url = async (data: Record<string, any>) => {
    const response = await axiosInstance.post('/api/v1/oauth2-authorize-url', data);
    return response.data;
};


export interface OAuth2Tokens {
    access_token: string;
    account_id: string;
    created_at: number;
    oauth2_name: string;
    refresh_token: string;
    updated_at: number;
}

export const get_oauth2_tokens = async (accountId: number) => {
    const response = await axiosInstance.get<OAuth2Tokens>(`/api/v1/oauth2-tokens/${accountId}`);
    return response.data;
};

export const exchange_client_credentials = async (accountId: number, oauth2Id: number) => {
    const response = await axiosInstance.post(`/api/v1/oauth2-exchange-client-credentials`, {
        account_id: accountId,
        oauth2_id: oauth2Id
    });
    return response.data;
};

export const refresh_oauth2_token = async (accountId: number) => {
    const response = await axiosInstance.post(`/api/v1/oauth2-tokens/${accountId}/refresh`);
    return response.data;
};