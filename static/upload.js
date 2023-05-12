function postFile() {
    var formdata = new FormData();
    formdata.append('FileToUpload', document.getElementById('FileToUpload').files[0]);
    var request = new XMLHttpRequest();

    request.upload.addEventListener('progress', function (e) {
        var file1Size = document.getElementById('FileToUpload').files[0].size;

        if (e.loaded <= file1Size) {
            var percent = Math.round(e.loaded / file1Size * 100);
            document.getElementById('ProgressBar').style.width = percent + '%';
            document.getElementById('ProgressBar').innerHTML = percent + '%';
        }

        if(e.loaded == e.total){
            document.getElementById('ProgressBar').style.width = '100%';
            document.getElementById('ProgressBar').innerHTML = '100%';
        }
    });

    request.open('post', '/upload/');
    request.timeout = 45000;
    request.send(formdata);
}
