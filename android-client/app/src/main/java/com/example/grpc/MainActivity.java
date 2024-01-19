package com.example.grpc;

import androidx.appcompat.app.AppCompatActivity;

import android.os.Bundle;
import android.util.Log;

public class MainActivity extends AppCompatActivity {

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Notification notification = new Notification();

        try {
            notification.streamPayload();
        } catch(Exception e){
            Log.i("GRPC", " [Error] : " + e.toString());
        }
    }
}