#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <stdlib.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec/1e9;}
static GLuint shader(GLenum type,const char *src){GLuint s=glCreateShader(type);glShaderSource(s,1,&src,NULL);glCompileShader(s);GLint ok;glGetShaderiv(s,GL_COMPILE_STATUS,&ok);if(!ok){char log[2048];glGetShaderInfoLog(s,sizeof(log),NULL,log);fprintf(stderr,"%s\n",log);exit(2);}return s;}
int main(int argc,char **argv){
 int duration=argc>1?atoi(argv[1]):20;if(duration<1||duration>60)return 2;
 EGLDisplay d=eglGetPlatformDisplay(EGL_PLATFORM_SURFACELESS_MESA,EGL_DEFAULT_DISPLAY,NULL);EGLint major,minor;
 if(!eglInitialize(d,&major,&minor)){fprintf(stderr,"EGL init failed %x\n",eglGetError());return 2;}
 eglBindAPI(EGL_OPENGL_ES_API);EGLint ca[]={EGL_SURFACE_TYPE,EGL_PBUFFER_BIT,EGL_RENDERABLE_TYPE,EGL_OPENGL_ES2_BIT,EGL_RED_SIZE,8,EGL_GREEN_SIZE,8,EGL_BLUE_SIZE,8,EGL_NONE};EGLConfig cfg;EGLint n;eglChooseConfig(d,ca,&cfg,1,&n);if(!n)return 2;
 EGLint pa[]={EGL_WIDTH,1024,EGL_HEIGHT,1024,EGL_NONE},ctxa[]={EGL_CONTEXT_CLIENT_VERSION,2,EGL_NONE};EGLSurface surf=eglCreatePbufferSurface(d,cfg,pa);EGLContext ctx=eglCreateContext(d,cfg,EGL_NO_CONTEXT,ctxa);if(!eglMakeCurrent(d,surf,surf,ctx))return 2;
 const char *renderer=(const char*)glGetString(GL_RENDERER);printf("GPU renderer: %s\n",renderer);fflush(stdout);if(!renderer||strstr(renderer,"llvmpipe")||strstr(renderer,"softpipe"))return 3;
 GLuint p=glCreateProgram();glAttachShader(p,shader(GL_VERTEX_SHADER,"attribute vec2 pos;void main(){gl_Position=vec4(pos,0.,1.);}"));glAttachShader(p,shader(GL_FRAGMENT_SHADER,"precision highp float;uniform float tick;void main(){vec2 z=gl_FragCoord.xy/1024.;float v=tick;for(int i=0;i<128;i++){v=sin(v+z.x)*cos(v+z.y)+0.1;}gl_FragColor=vec4(v,z.x,z.y,1.);}"));glBindAttribLocation(p,0,"pos");glLinkProgram(p);GLint ok;glGetProgramiv(p,GL_LINK_STATUS,&ok);if(!ok)return 2;glUseProgram(p);
 GLfloat v[]={-1,-1,1,-1,-1,1,1,1};glVertexAttribPointer(0,2,GL_FLOAT,GL_FALSE,0,v);glEnableVertexAttribArray(0);glViewport(0,0,1024,1024);GLint loc=glGetUniformLocation(p,"tick");double start=now(),last=start;int frames=0;
 while(now()-start<duration){glUniform1f(loc,(float)(now()-start));glDrawArrays(GL_TRIANGLE_STRIP,0,4);glFinish();frames++;if(now()-last>=2){printf("GPU load %.1fs/%ds frames=%d\n",now()-start,duration,frames);fflush(stdout);last=now();}}
 printf("Completed %d frames in %.2fs; GL error=%x\n",frames,now()-start,glGetError());eglMakeCurrent(d,EGL_NO_SURFACE,EGL_NO_SURFACE,EGL_NO_CONTEXT);eglDestroyContext(d,ctx);eglDestroySurface(d,surf);eglTerminate(d);return 0;
}
